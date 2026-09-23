# aiko CLI

Client Rust autonome pour la gateway HTTP d'Aiko, sur macOS et Linux. Il ne dépend pas du crate `aiko-core`.

## Installer

```sh
cargo install --path aiko-cli --locked
aiko --help
```

Depuis le répertoire `aiko-cli/` : `cargo install --path . --locked`. Une version de Rust compatible avec le `Cargo.lock` est nécessaire. Pour un VPS, copier le dépôt puis lancer cette même commande sur Linux ; le binaire est installé dans `~/.cargo/bin/aiko`.

Voir [la procédure complète](../docs/aiko-cli.md) pour les profils et un scénario terminal → tâche.

## Vérifier

```sh
cargo test --manifest-path aiko-cli/Cargo.toml --locked
cargo fmt --manifest-path aiko-cli/Cargo.toml -- --check
```
