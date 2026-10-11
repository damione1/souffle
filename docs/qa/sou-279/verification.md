# Vérification logicielle — SOU-279

Sources relues par Astra avec le correctif SOU-280 intégré : APPROVED.
Hash SHA-256 du delta indexé relu (hors ce journal) :
`5d4aa636e5b97671415ff7c9d4c23dcef83c69ffc5cc561f971d730c798c758d`.
Aucune capture micro, lecture haut-parleur ou transcription de données privées
n'a été exécutée par l'agent.

Environnement : arm64, cible `MACOSX_DEPLOYMENT_TARGET=13.0`, cache Cargo partagé
`CARGO_TARGET_DIR=/Users/damien/Projects/souffle/app/target`,
`CARGO_INCREMENTAL=0`. Les tentatives précédentes interrompues par manque
d'espace disque ne sont pas comptées : la gate complète ci-dessous a été
réexécutée après nettoyage des caches incrémentaux inactifs.

| Commande | Résultat |
|---|---|
| `./scripts/build-mcp-sidecar.test.sh` | PASS |
| `./scripts/build-mcp-sidecar.sh --debug` | PASS |
| `cargo fmt --manifest-path app/Cargo.toml --all -- --check` | PASS |
| `cargo clippy --manifest-path app/Cargo.toml --workspace --all-targets -- -D warnings` | PASS |
| `cargo test --manifest-path app/Cargo.toml --workspace --no-run` | PASS |
| `./scripts/ci/cargo-test-watchdog.sh 300 /tmp/sou279-gate-samples-default-final -- --manifest-path app/Cargo.toml --workspace` | PASS : 1622 tests, 0 échec, 22 ignorés |
| `./scripts/check_slint_translations.sh` | PASS EN/FR |
| `cargo clippy --manifest-path app/Cargo.toml --workspace --all-targets --features souffle-slint/experimental-native-aec -- -D warnings` | PASS |
| `cargo test --manifest-path app/Cargo.toml --workspace --features souffle-slint/experimental-native-aec --no-run` | PASS |
| `./scripts/ci/cargo-test-watchdog.sh 300 /tmp/sou279-gate-samples-feature-final -- --manifest-path app/Cargo.toml --workspace --features souffle-slint/experimental-native-aec` | PASS : 1624 tests, 0 échec, 22 ignorés |
| `bash -n scripts/bundle-macos.sh` | PASS |
| `./scripts/bundle-macos.sh --debug --native-aec` | Refus attendu avant build : `--nightly` requis |

Les nouveaux tests couvrent les rings natifs bornés, le choix compile-time,
le pin webcam absent malgré une résolution Auto interne, les changements de
route/remplacement micro sans FDAF, les pistes Me/Them distinctes et l'échec
de session conservé pendant la préparation ASR. Ce dernier teste aussi la
réponse Stop, une seconde session réussie sur le même canal, le rejet d'un
ancien signal d'erreur et la propagation d'une erreur au drain final.

Logs : `/tmp/sou279-gate-*-final.log`,
`/tmp/sou279-session-failure-test.log`, `/tmp/sou279-backend-tests.log`.
Cargo signale l'avertissement préexistant de compatibilité future `block 0.1.6` ;
le linker signale des objets Opus du cache construits avec une cible macOS 15.
La cible Rust/bundle est 13 ; aucune compatibilité physique macOS 13 n'est
prétendue par cette validation sur l'hôte courant.

## CodeQL et livraison QA

Le scan final des sources combinées est encore à exécuter après construction
du bundle QA. Aucune PR/push n'est autorisé avant sa validation. Le livrable QA
signé peut être remis pour les captures manuelles pendant ce scan.

Le scan du checkpoint SOU-278 `9a450ef6` a terminé avec une alerte Rust
`rust/cleartext-logging` sur l'affichage console de l'UID matériel du banc,
et zéro résultat JavaScript/Actions. Cet affichage redondant a été retiré dans
SOU-279 ; la preuve de périphérique reste dans le manifeste privé. L'archive
extraite a été comparée au checkpoint et les SARIF/hashes conservés dans
`/tmp/sou278-codeql-proof/`. Le scan SOU-278 ne valide pas les sources SOU-279.
