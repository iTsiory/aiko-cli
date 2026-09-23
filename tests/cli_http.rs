use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

struct Request {
    first_line: String,
    authorization: String,
    body: Value,
}

fn mock(status: u16, response: Value) -> (String, mpsc::Receiver<Request>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut raw = Vec::new();
        let header_end = loop {
            let mut buf = [0; 4096];
            let count = stream.read(&mut buf).unwrap();
            assert!(count > 0);
            raw.extend_from_slice(&buf[..count]);
            if let Some(pos) = raw.windows(4).position(|b| b == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let headers = String::from_utf8(raw[..header_end].to_vec()).unwrap();
        let length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        while raw.len() - header_end < length {
            let mut buf = [0; 4096];
            let count = stream.read(&mut buf).unwrap();
            assert!(count > 0);
            raw.extend_from_slice(&buf[..count]);
        }
        let body = if length == 0 {
            Value::Null
        } else {
            serde_json::from_slice(&raw[header_end..header_end + length]).unwrap()
        };
        tx.send(Request {
            first_line: headers.lines().next().unwrap().to_owned(),
            authorization: headers
                .lines()
                .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
                .unwrap_or("")
                .to_owned(),
            body,
        })
        .unwrap();
        let payload = response.to_string();
        write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}", payload.len()).unwrap();
    });
    (url, rx, handle)
}

fn run(url: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aiko"))
        .args(["--gateway-url", url, "--json"])
        .args(args)
        .env("AIKO_LOCAL_BEARER_TOKEN", "test-secret")
        .env(
            "AIKO_CONFIG_FILE",
            format!(
                "{}/aiko-cli-test-{}-{}.json",
                std::env::temp_dir().display(),
                std::process::id(),
                thread::current().name().unwrap_or("unnamed")
            ),
        )
        .output()
        .unwrap()
}

#[test]
fn status_and_registered_projects_use_bearer() {
    let (url, rx, server) = mock(200, json!({"ok":true,"version":"0.9.128"}));
    let output = run(&url, &["status"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["version"],
        "0.9.128"
    );
    let request = rx.recv().unwrap();
    assert_eq!(request.first_line, "GET /api/ping HTTP/1.1");
    assert_eq!(
        request.authorization.to_ascii_lowercase(),
        "authorization: bearer test-secret"
    );
    server.join().unwrap();

    let (url, rx, server) = mock(200, json!([{"id":1,"name":"demo","path":"/tmp/demo"}]));
    let output = run(&url, &["projects", "list"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        rx.recv().unwrap().first_line,
        "GET /api/projects/registered HTTP/1.1"
    );
    server.join().unwrap();
}

#[test]
fn task_create_and_project_list_preserve_contract() {
    let (url, rx, server) = mock(200, json!({"id":42}));
    let output = run(
        &url,
        &[
            "tasks",
            "create",
            "--card",
            "cterm:one",
            "--title",
            "Écrire",
            "--parent",
            "5",
            "--agent",
            "Maya",
            "--status",
            "in_progress",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().unwrap();
    assert_eq!(request.first_line, "POST /api/tasks HTTP/1.1");
    assert_eq!(
        request.body,
        json!({"card_key":"cterm:one","title":"Écrire","parent_id":5,"agent":"Maya","status":"in_progress"})
    );
    server.join().unwrap();

    let (url, rx, server) = mock(
        200,
        json!([{"id":42,"task_uuid":"abc","title":"Écrire","status":"pending"}]),
    );
    let output = run(&url, &["tasks", "list", "--project", "projet espace"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        rx.recv().unwrap().first_line,
        "GET /api/task-sync/tasks?project=projet+espace HTTP/1.1"
    );
    server.join().unwrap();
}

#[test]
fn mutations_and_sync_use_published_routes() {
    for (args, expected, body) in [
        (
            vec!["tasks", "update", "42", "--status", "in_progress"],
            "POST /api/tasks/update HTTP/1.1",
            json!({"id":42,"status":"in_progress"}),
        ),
        (
            vec!["tasks", "done", "42"],
            "POST /api/tasks/update HTTP/1.1",
            json!({"id":42,"status":"done"}),
        ),
        (
            vec!["tasks", "delete", "42"],
            "POST /api/tasks/delete HTTP/1.1",
            json!({"id":42}),
        ),
    ] {
        let (url, rx, server) = mock(200, json!({"ok":true}));
        let output = run(&url, &args);
        assert_eq!(output.status.code(), Some(0));
        let request = rx.recv().unwrap();
        assert_eq!(request.first_line, expected);
        assert_eq!(request.body, body);
        server.join().unwrap();
    }
    let status = json!({"enabled":true,"project":"demo","url":"https://cloud.example","last_sync":null,"error":null,"pending":1,"conflicts":0});
    let (url, rx, server) = mock(200, status.clone());
    let output = run(&url, &["cloud", "status"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        status
    );
    assert_eq!(
        rx.recv().unwrap().first_line,
        "GET /api/task-sync/status HTTP/1.1"
    );
    server.join().unwrap();
    let (url, rx, server) = mock(200, status);
    let output = run(&url, &["sync"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        rx.recv().unwrap().first_line,
        "POST /api/task-sync/run HTTP/1.1"
    );
    server.join().unwrap();

    let status = json!({"enabled":true,"project":"demo","url":"https://cloud.example","last_sync":null,"error":null,"pending":0,"conflicts":0,"cursor":null});
    let (url, rx, server) = mock(200, status);
    let output = Command::new(env!("CARGO_BIN_EXE_aiko"))
        .args([
            "--gateway-url",
            &url,
            "--json",
            "cloud",
            "configure",
            "--cloud-url",
            "https://cloud.example",
            "--project",
            "demo",
        ])
        .env("AIKO_LOCAL_BEARER_TOKEN", "test-secret")
        .env("AIKO_SYNC_TOKEN", "cloud-secret")
        .env(
            "AIKO_CONFIG_FILE",
            format!(
                "{}/aiko-cli-config-test-{}.json",
                std::env::temp_dir().display(),
                std::process::id()
            ),
        )
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("cloud-secret"));
    let request = rx.recv().unwrap();
    assert_eq!(request.first_line, "POST /api/task-sync/config HTTP/1.1");
    assert_eq!(
        request.body,
        json!({"url":"https://cloud.example","token":"cloud-secret","project":"demo","enabled":true})
    );
    server.join().unwrap();
}

#[test]
fn errors_have_stable_exit_codes_and_do_not_expose_token() {
    let (url, rx, server) = mock(401, json!({"error":"non autorisé"}));
    let output = run(&url, &["status"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("test-secret"));
    rx.recv().unwrap();
    server.join().unwrap();

    let output = run("http://127.0.0.1:1", &["status"]);
    assert_eq!(output.status.code(), Some(4));

    let output = run("http://198.51.100.7", &["status"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("HTTPS"));

    let output = run("http://127.0.0.1:1", &["tasks", "update", "42"]);
    assert_eq!(output.status.code(), Some(2));
    let output = run(
        "http://127.0.0.1:1",
        &["tasks", "create", "--card", "x", "--title", " "],
    );
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn profile_file_keeps_only_environment_variable_name() {
    let root = std::env::temp_dir().join(format!(
        "aiko-cli-profile-{}-{:?}",
        std::process::id(),
        thread::current().id()
    ));
    let config = root.join("config.json");
    let output = Command::new(env!("CARGO_BIN_EXE_aiko"))
        .args([
            "profile",
            "set",
            "cloud",
            "--url",
            "https://cloud.example",
            "--token-env",
            "MY_AIKO_SECRET",
        ])
        .env("AIKO_CONFIG_FILE", &config)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let saved: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
    assert_eq!(saved["profiles"]["cloud"]["url"], "https://cloud.example");
    assert_eq!(saved["profiles"]["cloud"]["token_env"], "MY_AIKO_SECRET");
    assert!(!saved.to_string().contains("cloud-secret"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&config).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_aiko"))
        .args(["--profile", "cloud", "--json", "profile", "show"])
        .env("AIKO_CONFIG_FILE", &config)
        .env("MY_AIKO_SECRET", "cloud-secret")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["token_env"],
        "MY_AIKO_SECRET"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("cloud-secret"));
    std::fs::remove_dir_all(root).unwrap();
}
