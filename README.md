# aiko CLI

Client Rust autonome pour la gateway HTTP d'[Aiko](https://aiko.youngdev.mg), sur macOS et Linux.

## Installer

```sh
cargo install --git https://github.com/iTsiory/aiko-cli --locked
# ou, depuis un clone :
cargo install --path . --locked
aiko --help
```

Une version de Rust compatible avec le `Cargo.lock` est nécessaire. Le binaire est installé dans `~/.cargo/bin/aiko`.

Voir [la procédure complète](docs/aiko-cli.md) pour les profils, les commandes et un scénario terminal → tâche.

## Vérifier

```sh
cargo test --locked
cargo fmt -- --check
```
