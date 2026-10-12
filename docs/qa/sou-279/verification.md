# Vérification logicielle — SOU-279

Le QA matériel et la fermeture du bundle signé `107140d4` sont consignés dans
[le rapport webcam du 11 octobre](hardware-qa-2026-10-11.md). Les résultats
ci-dessous décrivent d'abord le checkpoint `cddafd87` ; ils ne sont pas
attribués aux correctifs ABI AudioUnit et format mono ajoutés ensuite.

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
| `./scripts/ci/cargo-test-watchdog.sh 300 /tmp/sou279-gate-samples-default-cleanlog -- --manifest-path app/Cargo.toml --workspace` | PASS : 1622 tests, 0 échec, 22 ignorés |
| `./scripts/check_slint_translations.sh` | PASS EN/FR |
| `cargo clippy --manifest-path app/Cargo.toml --workspace --all-targets --features souffle-slint/experimental-native-aec -- -D warnings` | PASS |
| `cargo test --manifest-path app/Cargo.toml --workspace --features souffle-slint/experimental-native-aec --no-run` | PASS |
| `./scripts/ci/cargo-test-watchdog.sh 300 /tmp/sou279-gate-samples-feature-cleanlog -- --manifest-path app/Cargo.toml --workspace --features souffle-slint/experimental-native-aec` | PASS : 1624 tests, 0 échec, 22 ignorés |
| `bash -n scripts/bundle-macos.sh` | PASS |
| `./scripts/bundle-macos.sh --debug --native-aec` | Refus attendu avant build : `--nightly` requis |

Les nouveaux tests couvrent les rings natifs bornés, le choix compile-time,
le pin webcam absent malgré une résolution Auto interne, les changements de
route/remplacement micro sans FDAF, les pistes Me/Them distinctes et l'échec
de session conservé pendant la préparation ASR. Ce dernier teste aussi la
réponse Stop, une seconde session réussie sur le même canal, le rejet d'un
ancien signal d'erreur et la propagation d'une erreur au drain final.

La gate Rust/MCP/traductions complète a été réexécutée après la dernière
correction de sortie console du banc, elle aussi relue APPROVED par Astra.
Logs : `/tmp/sou279-gate-*-cleanlog.log`,
`/tmp/sou279-session-failure-test.log`, `/tmp/sou279-backend-tests.log`.
Cargo signale l'avertissement préexistant de compatibilité future `block 0.1.6` ;
le linker signale des objets Opus du cache construits avec une cible macOS 15.
La cible Rust/bundle est 13 ; aucune compatibilité physique macOS 13 n'est
prétendue par cette validation sur l'hôte courant.

## CodeQL et livraison QA

Le scan final des sources combinées et de la correction console est terminé :
**0 résultat Rust, 0 JavaScript, 0 GitHub Actions**, vérifiés dans les trois
SARIF après la sortie 0 du script. Commande exécutée avec l'environnement
Cargo/macOS ci-dessus, dans Bash :

```bash
codeql() {
    if [ "$1" = database ] && [ "$2" = analyze ]; then
        /opt/homebrew/bin/codeql "$@" --ram=24576 --max-disk-cache=8192
    else
        /opt/homebrew/bin/codeql "$@"
    fi
}
export -f codeql
CODEQL_THREADS=4 ./scripts/codeql-local.sh
```

Log : `/tmp/sou279-gate-codeql-cleanlog.log`. Les 14 fichiers Rust modifiés
depuis la base `15a1d826` ont été comparés octet pour octet à l'archive extraite.
SARIF, hashes et delta source conservés dans
`/tmp/sou279-codeql-cleanlog-proof/`. La limite de cache est indicative ; le
cache de résolution des types a temporairement dépassé 30 Gio. Aucun autre
scan n'a tourné en parallèle.

La Nightly QA a été construite avant cette dernière correction, qui ne change
que l'affichage du banc et ce journal. Son code applicatif reste celui relu,
testé et analysé. Commande :

```bash
./scripts/bundle-macos.sh --debug --nightly --native-aec
codesign --verify --deep --strict --verbose=2 \
  'app/target/debug/bundle/macos-native-aec/Soufflé Nightly.app'
```

Build et signature Developer ID : PASS, log `/tmp/sou279-nightly-build.log`.
Plist vérifié : `SouffleNativeAEC=true`,
`SouffleSourceCommit=c101189c2f290d35f1aa348bb033921cbda65d49`.
UUID Mach-O : `E9312F39-66E5-3B3D-8C96-04803A9B5602`.
SHA-256 Mach-O :
`41daba47aa5b751835ec95ae464e0d582fc64c759aff4e7778c3eca7f4fc4107`.
Les symboles `start_native_meeting` et `native_input_uid` sont présents.
Cette première livraison n'avait pas encore passé le QA acoustique. La seconde
livraison `107140d4`, testée par Damien, et ses limites sont décrites dans le
rapport lié en tête ; ces contrôles logiciels ne mesurent pas l'efficacité de
l'AEC.

Le scan du checkpoint SOU-278 `9a450ef6` a terminé avec une alerte Rust
`rust/cleartext-logging` sur l'affichage console de l'UID matériel du banc,
et zéro résultat JavaScript/Actions. Cet affichage redondant a été retiré dans
SOU-279 ; la preuve de périphérique reste dans le manifeste privé. L'archive
extraite a été comparée au checkpoint et les SARIF/hashes conservés dans
`/tmp/sou278-codeql-proof/`. Le scan SOU-278 ne valide pas les sources SOU-279.

Le premier scan SOU-279 à `c101189c` conservait une alerte au même affichage,
après retrait de l'UID explicite : le flux remontait à `Selection` via les
noms/fréquences affichés. Tous les champs `Selection` ont été retirés de cette
sortie redondante ; mode/scène restent affichés et les preuves de périphériques
restent dans le manifeste privé. Résultats de ce premier scan conservés dans
`/tmp/sou279-codeql-proof/` : Rust 1, JavaScript 0, Actions 0. Seul le scan de
confirmation ci-dessus valide la correction finale avec zéro alerte.

## Correctifs du bundle réellement testé

Commits applicatifs : `d86d1809` et `107140d4`. Les signatures `audioUnit`
Carbon et opaque sont vérifiées avant l'appel Objective-C, y compris en
release. Le graphe demande à Apple un flux client mono au taux du micro au
lieu de rejeter ou mélanger les neuf canaux du format initial VoiceProcessingIO.
Le format des deux côtés du graphe est vérifié avant et après démarrage,
puis pendant le fonctionnement ; les buffers incompatibles sont refusés.
Les erreurs de capture conservent leur diagnostic et une catégorie distincte
des erreurs modèle dans la bibliothèque et l'UI.

Revue Astra du delta complet `cddafd87..107140d4` : **APPROVED**, aucun finding
bloquant. Vérification des headers Apple, de l'encodage/ownership FFI, des
formats et callbacks, des pins/readbacks, du teardown, du banc et de la
propagation exhaustive des diagnostics.

| Commande sur les sources `107140d4` | Résultat |
|---|---|
| `cargo clippy --manifest-path app/Cargo.toml --workspace --all-targets --features souffle-slint/experimental-native-aec -- -D warnings` | PASS, log `/tmp/sou277-mono-clippy-final.log` |
| `cargo test --manifest-path app/Cargo.toml --workspace --features souffle-slint/experimental-native-aec` | PASS : 1631 tests, 0 échec, 24 ignorés ; log `/tmp/sou277-mono-workspace-tests-final.log` |
| `SOUFFLE_NATIVE_TEST_INPUT_UID=<webcam-uid> cargo test --manifest-path app/Cargo.toml -p souffle --features native-voice stopped_voice_io_negotiates_mono_client_graph -- --ignored --nocapture --test-threads=1` | PASS : graphe arrêté, pas de capture ; log `/tmp/sou277-mono-stopped-smoke.log` |
| `./scripts/bundle-macos.sh --debug --nightly --native-aec --dmg` | PASS : app signée et image QA ; log `/tmp/sou277-mono-nightly-build.log` |
| `codesign --verify --deep --strict --verbose=2 '/Applications/Soufflé Nightly.app'` | PASS après les trois prises et ⌘Q |

La gate finale a été réexécutée sur ces sources après les correctifs. Les
résultats du checkpoint `cddafd87` ne remplacent pas ce nouveau scan CodeQL.

| Gate complète actualisée | Résultat |
|---|---|
| `./scripts/build-mcp-sidecar.test.sh` | PASS |
| `./scripts/build-mcp-sidecar.sh --debug` | PASS |
| `cargo fmt --manifest-path app/Cargo.toml --all -- --check` | PASS |
| `cargo clippy --manifest-path app/Cargo.toml --workspace --all-targets -- -D warnings` | PASS |
| `cargo test --manifest-path app/Cargo.toml --workspace --no-run` | PASS |
| `./scripts/ci/cargo-test-watchdog.sh 300 /tmp/sou277-qa-default-samples -- --manifest-path app/Cargo.toml --workspace` | PASS : 1629 tests, 0 échec, 24 ignorés |
| `./scripts/check_slint_translations.sh` | PASS EN/FR |

Logs de cette gate : `/tmp/sou277-qa-*.log`. Les configurations standard et
Apple passent respectivement 1629 et 1631 tests ; les 24 tests ignorés incluent
des essais matériels réservés à un lancement explicite. Le test du graphe
Apple arrêté ci-dessus a été exécuté séparément. Les avertissements préexistants
`block 0.1.6` et linker `__eh_frame` ne sont pas présentés comme des échecs.

### CodeQL final des sources `107140d4`

Le script est terminé avec le code de sortie **0**. Les trois SARIF contiennent
des runs valides ; tous leurs résultats ont été comptés indépendamment avec
`jq '[.runs[].results[]] | length'` : **Rust 0, JavaScript 0, Actions 0**.
Les 24 fichiers Rust modifiés depuis `origin/develop` (`15a1d826`) ont été
comparés octet pour octet à l'archive `rust/src.zip` de cette extraction.

Commande dans Bash, avec l'environnement Cargo/macOS ci-dessus :

```bash
codeql() {
    if [ "$1" = database ] && [ "$2" = analyze ]; then
        /opt/homebrew/bin/codeql "$@" --ram=16384 --max-disk-cache=8192
    else
        /opt/homebrew/bin/codeql "$@"
    fi
}
export -f codeql
CODEQL_THREADS=4 ./scripts/codeql-local.sh
```

Log : `/tmp/sou277-qa-codeql.log`. SARIF, hashes de sources et d'archive,
commit analysé et base conservés dans `/tmp/sou277-qa-codeql-proof/`.
Les changements suivants ne concernent que la documentation QA et le guide
du format mono ; les sources applicatives testées et analysées restent celles
de `107140d4`. Aucun scan incomplet antérieur n'est compté comme cette preuve.
