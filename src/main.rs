use clap::{Parser, Subcommand, ValueEnum};
use reqwest::blocking::Client;
use reqwest::{Method, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "aiko",
    version,
    about = "Piloter une gateway Aiko depuis un terminal"
)]
struct Cli {
    #[arg(long, global = true, value_enum, default_value = "local")]
    profile: ProfileName,
    #[arg(
        long = "gateway-url",
        global = true,
        help = "URL de la gateway pour cet appel"
    )]
    gateway_url: Option<String>,
    #[arg(
        long = "gateway-token-env",
        global = true,
        help = "Nom de la variable contenant le jeton Bearer"
    )]
    gateway_token_env: Option<String>,
    #[arg(
        long,
        global = true,
        help = "Autoriser HTTP avec authentification hors loopback"
    )]
    allow_http: bool,
    #[arg(long, global = true, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=300))]
    timeout: u64,
    #[arg(long, global = true, help = "Afficher la réponse JSON brute")]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum ProfileName {
    Local,
    Cloud,
}

impl ProfileName {
    fn key(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Cloud => "cloud",
        }
    }

    fn default_token_env(self) -> &'static str {
        match self {
            Self::Local => "AIKO_LOCAL_BEARER_TOKEN",
            Self::Cloud => "AIKO_CLOUD_BEARER_TOKEN",
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Vérifier la gateway active
    Status,
    /// Afficher ou configurer un profil sans enregistrer de secret
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    /// Lister les projets enregistrés
    Projects {
        #[command(subcommand)]
        command: ProjectsCommand,
    },
    /// Gérer les tâches des cartes
    Tasks {
        #[command(subcommand)]
        command: TasksCommand,
    },
    /// État de la synchronisation configurée sur la gateway
    Cloud {
        #[command(subcommand)]
        command: CloudCommand,
    },
    /// Déclencher la synchronisation configurée sur la gateway
    Sync,
}

#[derive(Subcommand)]
enum ProfileCommand {
    Show,
    Set {
        name: ProfileName,
        #[arg(long)]
        url: String,
        #[arg(long)]
        token_env: Option<String>,
    },
}

#[derive(Subcommand)]
enum ProjectsCommand {
    List,
}

#[derive(Subcommand)]
enum CloudCommand {
    Status,
    /// Configurer la synchronisation sur la gateway sélectionnée
    Configure {
        #[arg(long = "cloud-url")]
        url: String,
        #[arg(long)]
        project: String,
        #[arg(long, default_value = "AIKO_SYNC_TOKEN")]
        token_env: String,
        #[arg(long)]
        disabled: bool,
    },
}

#[derive(Subcommand)]
enum TasksCommand {
    List {
        #[arg(long, conflicts_with = "project")]
        card: Option<String>,
        #[arg(long)]
        project: Option<String>,
    },
    Create {
        #[arg(long)]
        card: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        parent: Option<i64>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, value_enum)]
        status: Option<TaskStatus>,
    },
    Update {
        id: i64,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, value_enum)]
        status: Option<TaskStatus>,
    },
    Done {
        id: i64,
    },
    Delete {
        id: i64,
    },
}

#[derive(Clone, Copy, ValueEnum)]
#[value(rename_all = "snake_case")]
enum TaskStatus {
    Pending,
    InProgress,
    Done,
    Cancelled,
}

impl TaskStatus {
    fn key(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Config {
    #[serde(default)]
    profiles: BTreeMap<String, ProfileConfig>,
}

#[derive(Default, Serialize, Deserialize)]
struct ProfileConfig {
    url: String,
    #[serde(default)]
    token_env: Option<String>,
}

struct Gateway {
    client: Client,
    base: Url,
    token: String,
}

#[derive(Debug)]
struct CliError {
    code: u8,
    message: String,
}

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            code: 2,
            message: message.into(),
        }
    }

    fn auth(message: impl Into<String>) -> Self {
        Self {
            code: 3,
            message: message.into(),
        }
    }

    fn network(message: impl Into<String>) -> Self {
        Self {
            code: 4,
            message: message.into(),
        }
    }

    fn server(message: impl Into<String>) -> Self {
        Self {
            code: 5,
            message: message.into(),
        }
    }

    fn protocol(message: impl Into<String>) -> Self {
        Self {
            code: 6,
            message: message.into(),
        }
    }
}

fn nonempty(value: &str, field: &str) -> Result<(), CliError> {
    if value.trim().is_empty() {
        Err(CliError::usage(format!("{field} ne peut pas être vide")))
    } else {
        Ok(())
    }
}

fn positive(id: i64, field: &str) -> Result<(), CliError> {
    if id > 0 {
        Ok(())
    } else {
        Err(CliError::usage(format!("{field} doit être positif")))
    }
}

fn config_path() -> Result<PathBuf, CliError> {
    if let Some(path) = env::var_os("AIKO_CONFIG_FILE") {
        return Ok(PathBuf::from(path));
    }
    let home = env::var_os("HOME").ok_or_else(|| CliError::usage("HOME est absent"))?;
    Ok(PathBuf::from(home).join(".config/aiko/config.json"))
}

fn read_config(path: &Path) -> Result<Config, CliError> {
    match fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content)
            .map_err(|e| CliError::usage(format!("Configuration invalide : {e}"))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(CliError::usage(format!("Configuration illisible : {e}"))),
    }
}

fn write_config(path: &Path, config: &Config) -> Result<(), CliError> {
    let parent = path
        .parent()
        .ok_or_else(|| CliError::usage("Chemin de configuration invalide"))?;
    let created_parent = !parent.exists();
    fs::create_dir_all(parent).map_err(|e| CliError::usage(format!("Configuration : {e}")))?;
    #[cfg(unix)]
    if created_parent {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .map_err(|e| CliError::usage(format!("Permissions du dossier : {e}")))?;
    }
    let content =
        serde_json::to_vec_pretty(config).map_err(|e| CliError::protocol(e.to_string()))?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp)
        .map_err(|e| CliError::usage(format!("Configuration : {e}")))?;
    file.write_all(&content)
        .map_err(|e| CliError::usage(format!("Configuration : {e}")))?;
    file.sync_all()
        .map_err(|e| CliError::usage(format!("Configuration : {e}")))?;
    fs::rename(&temp, path).map_err(|e| CliError::usage(format!("Configuration : {e}")))?;
    Ok(())
}

fn parse_url(raw: &str) -> Result<Url, CliError> {
    let url = Url::parse(raw).map_err(|_| CliError::usage("URL de gateway invalide"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(CliError::usage(
            "URL attendue : http(s)://hôte[:port][/préfixe], sans identifiants ni query",
        ));
    }
    Ok(url)
}

fn is_loopback(url: &Url) -> bool {
    match url.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
        None => false,
    }
}

fn resolved_profile(cli: &Cli, config: &Config) -> Result<(Url, String), CliError> {
    let selected = config.profiles.get(cli.profile.key());
    let raw = cli
        .gateway_url
        .as_deref()
        .or_else(|| selected.map(|p| p.url.as_str()))
        .or_else(|| matches!(cli.profile, ProfileName::Local).then_some("http://127.0.0.1:4823"))
        .ok_or_else(|| {
            CliError::usage("Profil cloud sans URL : aiko profile set cloud --url https://…")
        })?;
    let url = parse_url(raw)?;
    if url.scheme() == "http" && !is_loopback(&url) && !cli.allow_http {
        return Err(CliError::usage(
            "HTTP authentifié hors loopback refusé ; utiliser HTTPS ou --allow-http",
        ));
    }
    let env_name = cli
        .gateway_token_env
        .as_deref()
        .or_else(|| selected.and_then(|p| p.token_env.as_deref()))
        .unwrap_or(cli.profile.default_token_env());
    nonempty(env_name, "Nom de variable du jeton")?;
    let token = env::var(env_name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            if matches!(cli.profile, ProfileName::Local)
                && cli.gateway_token_env.is_none()
                && selected.and_then(|p| p.token_env.as_ref()).is_none()
            {
                env::var("AIKO_REMOTE_TOKEN")
                    .ok()
                    .filter(|v| !v.trim().is_empty())
                    .or_else(|| {
                        env::var_os("HOME")
                            .and_then(|home| {
                                fs::read_to_string(
                                    PathBuf::from(home).join("aiko-agent/.remote-token"),
                                )
                                .ok()
                            })
                            .map(|v| v.trim().to_owned())
                            .filter(|v| !v.is_empty())
                    })
            } else {
                None
            }
        })
        .ok_or_else(|| {
            CliError::auth(format!(
                "Jeton absent : définir {env_name}{}",
                if matches!(cli.profile, ProfileName::Local) {
                    " ou ~/aiko-agent/.remote-token"
                } else {
                    ""
                }
            ))
        })?;
    Ok((url, token))
}

impl Gateway {
    fn new(cli: &Cli, config: &Config) -> Result<Self, CliError> {
        let (mut base, token) = resolved_profile(cli, config)?;
        let path = format!("{}/", base.path().trim_end_matches('/'));
        base.set_path(&path);
        let client = Client::builder()
            .timeout(Duration::from_secs(cli.timeout))
            .connect_timeout(Duration::from_secs(cli.timeout.min(5)))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| CliError::network(e.to_string()))?;
        Ok(Self {
            client,
            base,
            token,
        })
    }

    fn request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<Value, CliError> {
        let mut url = self
            .base
            .join(path.trim_start_matches('/'))
            .map_err(|_| CliError::usage("Chemin de requête invalide"))?;
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query.iter().copied());
        }
        let mut req = self.client.request(method, url).bearer_auth(&self.token);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let response = req.send().map_err(|e| {
            CliError::network(if e.is_timeout() {
                "Délai d'attente dépassé".to_owned()
            } else {
                format!("Gateway inaccessible : {e}")
            })
        })?;
        let status = response.status();
        let bytes = response
            .bytes()
            .map_err(|e| CliError::network(e.to_string()))?;
        let value: Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) if !status.is_success() => {
                return Err(CliError::server(format!(
                    "HTTP {status} : réponse non JSON"
                )))
            }
            Err(_) => {
                return Err(CliError::protocol(format!(
                    "Réponse JSON invalide (HTTP {status})"
                )))
            }
        };
        if !status.is_success() {
            let detail = value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Erreur de la gateway");
            let message = format!("HTTP {status} : {detail}");
            return Err(if matches!(status.as_u16(), 401 | 403) {
                CliError::auth(message)
            } else {
                CliError::server(message)
            });
        }
        Ok(value)
    }
}

fn expect_array<'a>(value: &'a Value, route: &str) -> Result<&'a Vec<Value>, CliError> {
    value
        .as_array()
        .ok_or_else(|| CliError::protocol(format!("{route} : tableau attendu")))
}

fn expect_ok(value: &Value) -> Result<(), CliError> {
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(CliError::protocol("Réponse sans ok=true"))
    }
}

fn display(cli: &Cli, value: Value, kind: &str) -> Result<(), CliError> {
    match kind {
        "status" if value.get("ok").and_then(Value::as_bool) != Some(true) => {
            return Err(CliError::protocol("/api/ping : ok=true attendu"))
        }
        "projects" | "tasks" => {
            expect_array(&value, kind)?;
        }
        "created"
            if value
                .get("id")
                .and_then(Value::as_i64)
                .is_none_or(|id| id <= 0) =>
        {
            return Err(CliError::protocol("Réponse sans id positif"))
        }
        "ok" => expect_ok(&value)?,
        "sync" if value.get("enabled").and_then(Value::as_bool).is_none() => {
            return Err(CliError::protocol("Statut sync sans enabled"))
        }
        _ => {}
    }
    if cli.json {
        println!(
            "{}",
            serde_json::to_string(&value).map_err(|e| CliError::protocol(e.to_string()))?
        );
        return Ok(());
    }
    match kind {
        "status" => {
            println!(
                "Aiko {} · gateway disponible",
                value
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or("version inconnue")
            );
        }
        "projects" => {
            for project in expect_array(&value, "/api/projects/registered")? {
                let name = project
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CliError::protocol("Projet sans nom"))?;
                println!(
                    "{name}\t{}",
                    project.get("path").and_then(Value::as_str).unwrap_or("")
                );
            }
        }
        "tasks" => {
            for task in expect_array(&value, "liste de tâches")? {
                let id = task
                    .get("id")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| CliError::protocol("Tâche sans id local"))?;
                let title = task
                    .get("title")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CliError::protocol("Tâche sans titre"))?;
                let uuid = task
                    .get("task_uuid")
                    .and_then(Value::as_str)
                    .map(|v| format!("\t{v}"))
                    .unwrap_or_default();
                println!(
                    "{id}\t{}\t{title}{uuid}",
                    task.get("status").and_then(Value::as_str).unwrap_or("?")
                );
            }
        }
        "created" => println!("Tâche créée : {}", value["id"]),
        "ok" => println!("OK"),
        "sync" => {
            let enabled = value["enabled"].as_bool().unwrap();
            println!(
                "Synchronisation : {}",
                if enabled { "active" } else { "inactive" }
            );
            for field in [
                "project",
                "url",
                "last_sync",
                "pending",
                "conflicts",
                "error",
            ] {
                if let Some(entry) = value.get(field).filter(|v| !v.is_null()) {
                    println!(
                        "{field} : {}",
                        entry
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| entry.to_string())
                    );
                }
            }
        }
        _ => return Err(CliError::protocol("Affichage inconnu")),
    }
    Ok(())
}

fn run(cli: &Cli) -> Result<(), CliError> {
    let path = config_path()?;
    let mut config = read_config(&path)?;
    if let Command::Profile { command } = &cli.command {
        return match command {
            ProfileCommand::Show => {
                let profile = config.profiles.get(cli.profile.key());
                let value = json!({"profile": cli.profile.key(),
                    "url": cli.gateway_url.as_deref().or_else(|| profile.map(|p| p.url.as_str())).unwrap_or(if matches!(cli.profile, ProfileName::Local) { "http://127.0.0.1:4823" } else { "" }),
                    "token_env": cli.gateway_token_env.as_deref().or_else(|| profile.and_then(|p| p.token_env.as_deref())).unwrap_or(cli.profile.default_token_env())});
                if cli.json {
                    println!("{value}");
                } else {
                    println!(
                        "{}\nURL : {}\nJeton : variable {}",
                        cli.profile.key(),
                        value["url"].as_str().unwrap_or(""),
                        value["token_env"].as_str().unwrap_or("")
                    );
                }
                Ok(())
            }
            ProfileCommand::Set {
                name,
                url,
                token_env,
            } => {
                parse_url(url)?;
                if let Some(name) = token_env {
                    nonempty(name, "Nom de variable du jeton")?;
                }
                config.profiles.insert(
                    name.key().to_owned(),
                    ProfileConfig {
                        url: url.clone(),
                        token_env: token_env.clone(),
                    },
                );
                write_config(&path, &config)?;
                if cli.json {
                    println!("{}", json!({"ok":true,"profile":name.key()}));
                } else {
                    println!("Profil {} enregistré", name.key());
                }
                Ok(())
            }
        };
    }

    // Les options sont validées avant toute requête réseau.
    match &cli.command {
        Command::Tasks { command } => match command {
            TasksCommand::List { card, project } => {
                if card.is_none() && project.is_none() {
                    return Err(CliError::usage("Choisir --card ou --project"));
                }
                nonempty(
                    card.as_deref().or(project.as_deref()).unwrap(),
                    "Carte/projet",
                )?;
            }
            TasksCommand::Create {
                card,
                title,
                parent,
                agent,
                ..
            } => {
                nonempty(card, "Carte")?;
                nonempty(title, "Titre")?;
                if let Some(id) = parent {
                    positive(*id, "Parent")?;
                }
                if let Some(agent) = agent {
                    nonempty(agent, "Agent")?;
                }
            }
            TasksCommand::Update {
                id,
                title,
                agent,
                status,
            } => {
                positive(*id, "ID")?;
                if title.is_none() && agent.is_none() && status.is_none() {
                    return Err(CliError::usage("Donner --title, --agent ou --status"));
                }
                if let Some(title) = title {
                    nonempty(title, "Titre")?;
                }
                if let Some(agent) = agent {
                    nonempty(agent, "Agent")?;
                }
            }
            TasksCommand::Done { id } | TasksCommand::Delete { id } => positive(*id, "ID")?,
        },
        Command::Cloud {
            command:
                CloudCommand::Configure {
                    url,
                    project,
                    token_env,
                    ..
                },
        } => {
            nonempty(project, "Projet")?;
            nonempty(token_env, "Nom de variable du jeton cloud")?;
            let url = parse_url(url)?;
            if url.scheme() == "http" && !is_loopback(&url) && !cli.allow_http {
                return Err(CliError::usage(
                    "HTTP cloud hors loopback refusé ; utiliser HTTPS ou --allow-http",
                ));
            }
        }
        _ => {}
    }

    let gateway = Gateway::new(cli, &config)?;
    match &cli.command {
        Command::Status => display(
            cli,
            gateway.request(Method::GET, "api/ping", &[], None)?,
            "status",
        ),
        Command::Projects {
            command: ProjectsCommand::List,
        } => display(
            cli,
            gateway.request(Method::GET, "api/projects/registered", &[], None)?,
            "projects",
        ),
        Command::Tasks { command } => match command {
            TasksCommand::List {
                card: Some(card), ..
            } => display(
                cli,
                gateway.request(Method::GET, "api/tasks", &[("card_key", card)], None)?,
                "tasks",
            ),
            TasksCommand::List {
                project: Some(project),
                ..
            } => display(
                cli,
                gateway.request(
                    Method::GET,
                    "api/task-sync/tasks",
                    &[("project", project)],
                    None,
                )?,
                "tasks",
            ),
            TasksCommand::List { .. } => unreachable!(),
            TasksCommand::Create {
                card,
                title,
                parent,
                agent,
                status,
            } => {
                let mut body = json!({"card_key":card,"title":title});
                if let Some(parent) = parent {
                    body["parent_id"] = json!(parent);
                }
                if let Some(agent) = agent {
                    body["agent"] = json!(agent);
                }
                if let Some(status) = status {
                    body["status"] = json!(status.key());
                }
                display(
                    cli,
                    gateway.request(Method::POST, "api/tasks", &[], Some(body))?,
                    "created",
                )
            }
            TasksCommand::Update {
                id,
                title,
                agent,
                status,
            } => {
                let mut body = json!({"id":id});
                if let Some(title) = title {
                    body["title"] = json!(title);
                }
                if let Some(agent) = agent {
                    body["agent"] = json!(agent);
                }
                if let Some(status) = status {
                    body["status"] = json!(status.key());
                }
                display(
                    cli,
                    gateway.request(Method::POST, "api/tasks/update", &[], Some(body))?,
                    "ok",
                )
            }
            TasksCommand::Done { id } => display(
                cli,
                gateway.request(
                    Method::POST,
                    "api/tasks/update",
                    &[],
                    Some(json!({"id":id,"status":"done"})),
                )?,
                "ok",
            ),
            TasksCommand::Delete { id } => display(
                cli,
                gateway.request(
                    Method::POST,
                    "api/tasks/delete",
                    &[],
                    Some(json!({"id":id})),
                )?,
                "ok",
            ),
        },
        Command::Cloud {
            command: CloudCommand::Status,
        } => display(
            cli,
            gateway.request(Method::GET, "api/task-sync/status", &[], None)?,
            "sync",
        ),
        Command::Cloud {
            command:
                CloudCommand::Configure {
                    url,
                    project,
                    token_env,
                    disabled,
                },
        } => {
            let mut body = json!({"url":url,"project":project,"enabled":!disabled});
            if let Some(token) = env::var(token_env).ok().filter(|v| !v.trim().is_empty()) {
                body["token"] = json!(token);
            }
            let response =
                gateway.request(Method::POST, "api/task-sync/config", &[], Some(body))?;
            display(cli, response, "sync")
        }
        Command::Sync => display(
            cli,
            gateway.request(Method::POST, "api/task-sync/run", &[], Some(json!({})))?,
            "sync",
        ),
        Command::Profile { .. } => unreachable!(),
    }
}

fn main() {
    let cli = Cli::parse();
    if let Err(error) = run(&cli) {
        if cli.json {
            eprintln!("{}", json!({"error":error.message,"code":error.code}));
        } else {
            eprintln!("aiko : {}", error.message);
        }
        std::process::exit(error.code.into());
    }
}
