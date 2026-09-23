# CLI `aiko`

Le binaire `aiko` parle à la gateway HTTP du backend Aiko. Les commandes `tasks` utilisent les tâches des cartes, distinctes des TODO du jour. Les IDs numériques des tâches appartiennent à la base SQLite de **la gateway ciblée** : ne les réutilisez pas sur une autre instance. Le champ `task_uuid`, s'il est fourni, est conservé dans la sortie `--json` pour la synchronisation.

## Installation macOS et Linux/VPS

```sh
cargo install --git https://github.com/iTsiory/aiko-cli --locked
aiko --help
```

L'installation produit `~/.cargo/bin/aiko`. Ajouter `~/.cargo/bin` au `PATH` si nécessaire. Sur un VPS Debian/Ubuntu, installer Rust et les bibliothèques de compilation TLS (`pkg-config`, `libssl-dev`), puis exécuter la même commande.

## Profils et secrets

`local` cible par défaut `http://127.0.0.1:4823`. Son jeton Bearer vient de `AIKO_LOCAL_BEARER_TOKEN`, puis de `AIKO_REMOTE_TOKEN`, puis de `$AIKO_CLOUD_HOME/aiko-agent/.remote-token` si `AIKO_CLOUD_HOME` est défini, puis de `~/aiko-agent/.remote-token`. Sur le VPS, le service a pour `HOME` `AIKO_CLOUD_HOME` (par défaut systemd : `/var/lib/aiko-cloud`) et crée ce jeton en mode `0600`. `AIKO_LOCAL_TOKEN` / `.local-token` est un autre secret et ne convient pas au Bearer.

```sh
aiko profile set local --url http://127.0.0.1:4823
aiko profile set cloud --url https://aiko.example.org --token-env AIKO_CLOUD_BEARER_TOKEN
export AIKO_CLOUD_BEARER_TOKEN='…'
aiko --profile cloud status
aiko --profile cloud projects list
```

Depuis un poste distant, ouvrir un tunnel `ssh -L 4824:127.0.0.1:4823 utilisateur@vps`, puis configurer `cloud` avec `http://127.0.0.1:4824` et le Bearer du VPS dans `AIKO_CLOUD_BEARER_TOKEN`. Un proxy HTTPS vers la gateway du VPS convient aussi. La gateway du VPS écoute `127.0.0.1:4823` par défaut ; le CLI n'attend aucune route cloud propre au runtime. `aiko status` vérifie `/api/ping` ; `aiko cloud status` lit l'état de la **synchronisation des tâches** fourni par la gateway locale.

Le fichier `~/.config/aiko/config.json` ne contient que les URL et les **noms** des variables de jeton. Sur Unix, le CLI crée le dossier en mode `0700` et le fichier en mode `0600`. `AIKO_CONFIG_FILE` déplace ce fichier. Les options globales `--gateway-url` et `--gateway-token-env` remplacent le profil pour un appel. `aiko --profile cloud profile show` affiche la configuration sans afficher le jeton.

Le CLI refuse une URL HTTP hors loopback pour les appels authentifiés. Un réseau privé qui termine TLS ailleurs peut être utilisé avec `--allow-http` explicite. Les URL contenant un identifiant, un mot de passe ou une query sont refusées. Le délai total est de 10 secondes par défaut (`--timeout 1..300`). Le jeton n'est jamais passé comme argument ni imprimé.

## Commandes

```sh
aiko status
aiko projects list
aiko tasks list --card 'cterm-codex:…'
aiko tasks list --project 'mon-projet'
aiko tasks create --project mon-projet --card 'terminal-vps' --title 'Préparer le déploiement' --agent Maya
aiko tasks create --project mon-projet --card 'terminal-vps' --title 'Vérifier le VPS' --parent 42 --status in_progress
aiko tasks update 42 --title 'Préparer le VPS' --status in_progress
aiko tasks done 42
aiko tasks delete 42
aiko cloud status
aiko sync
aiko cloud conflicts
aiko cloud resolve 0123456789abcdef0123456789abcdef --strategy local
```

`tasks list --card` appelle `GET /api/tasks?card_key=…`. `tasks list --project` appelle `GET /api/task-sync/tasks?project=…` pour inclure les cartes absentes. `tasks create --project` transmet le nom d'un projet enregistré à `POST /api/tasks` : utilisez-le pour une clé de terminal VPS qui n'est pas un chemin de carte desktop. Sans lui, les anciennes gateways infèrent le projet depuis le chemin `card_key`, et une clé arbitraire peut créer une tâche hors projet qui ne sera pas synchronisée. Mise à jour, fin et suppression restent sur les routes `/api/tasks*` existantes et exigent l'ID **local** pour les mutations. Les statuts acceptés sont `pending`, `in_progress`, `done`, `cancelled`. Les options sont validées avant l'appel HTTP.

Avant la première tâche sur un serveur neuf, enregistrer le projet par la route réelle de la gateway (le dossier doit exister et être accessible au service). Par exemple, sur le VPS, après avoir placé le Bearer dans `AIKO_LOCAL_BEARER_TOKEN` :

```sh
sudo -u aiko-cloud mkdir -p /var/lib/aiko-cloud/projects/mon-projet
curl -fsS -H "Authorization: Bearer $AIKO_LOCAL_BEARER_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"path":"/var/lib/aiko-cloud/projects/mon-projet"}' \
  http://127.0.0.1:4823/api/project/import
aiko projects list
```

Le Mac et le VPS doivent enregistrer le **même nom** de projet ; leurs chemins locaux peuvent différer.

La synchronisation est configurée **sur la gateway ciblée** :

```sh
export AIKO_SYNC_TOKEN='…'
aiko cloud configure --cloud-url https://cloud.example.org --project mon-projet --token-env AIKO_SYNC_TOKEN
aiko cloud status
aiko sync
```

`cloud status`, `cloud configure` et `sync` utilisent respectivement `GET /api/task-sync/status`, `POST /api/task-sync/config` et `POST /api/task-sync/run`. Une gateway qui ne publie pas ces routes renvoie une erreur HTTP ; le CLI ne simule aucune synchronisation. `--profile cloud` choisit **la gateway appelée** ; `cloud configure --cloud-url` désigne **le serveur avec lequel cette gateway synchronise**. Le jeton cloud est facultatif pour modifier une configuration existante : si `AIKO_SYNC_TOKEN` est absent, le CLI omet `token` dans la requête.

`aiko cloud conflicts` lit `GET /api/task-sync/conflicts` et montre les versions `local` et `remote` avec leur UUID. `aiko cloud resolve <uuid> --strategy local|remote` appelle `POST /api/task-sync/resolve`. Choisir `local` garde l'édition locale et la publie au prochain cycle ; choisir `remote` adopte immédiatement la version du VPS. Utiliser `--json` pour comparer tous les champs sans formatage, puis `aiko sync` si l'on veut lancer le cycle sans attendre le worker. La résolution exige un choix explicite pour chaque conflit.

## Sortie et erreurs

`--json` affiche la réponse de la gateway sans perdre ses champs additionnels. En cas d'échec, le message JSON va sur stderr. Codes de sortie : `0` succès, `2` options/configuration invalides, `3` jeton absent ou HTTP 401/403, `4` réseau/délai, `5` HTTP autre, `6` réponse JSON ou contrat invalide. Les erreurs et l'aide vont sur stderr ; les données vont sur stdout.

## Scénario terminal → tâche

1. Démarrer la gateway locale et vérifier `aiko status`.
2. Identifier une carte dans Aiko, puis lancer `aiko tasks list --card '<clé>'`.
3. Créer une tâche : `aiko tasks create --project <nom> --card '<clé>' --title 'Vérifier le CLI'` ; noter l'ID local rendu.
4. Terminer la tâche avec `aiko tasks done <id>` et relister la carte.
5. Une fois `/api/task-sync/*` disponible et configuré, lancer `aiko sync`, puis `aiko tasks list --project <nom> --json` pour consulter `task_uuid`.
