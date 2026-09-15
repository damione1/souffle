# Spike Report: Coquille Slint sans Tauri (SOU-186)

## Objectifs
Évaluer la faisabilité et les gains de performance d'une interface Slint autonome (sans Tauri) pour l'application Soufflé sur macOS, avec prise en charge du menu natif et de VoiceOver.

## Implémentation
- **Binaire autonome** : Création du point d'entrée `souffle-slint` (`src/bin/souffle-slint.rs`) utilisant `slint` et `slint-build`.
- **Menu natif AppKit** : Implémenté en Rust via `objc2` (messages Objective-C `msg_send!`) sans surcharger les dépendances (réutilise `objc2` et `objc2-foundation` déjà dans `Cargo.toml`). Prise en charge du menu App (Quit ⌘Q) et Edit (Cut, Copy, Paste, Select All).
- **Dock & Activation Policy** : Intégration de `NSApplicationActivationPolicyRegular` permettant à l'application d'apparaître dans le Dock et Cmd-Tab.
- **Packaging** : Script bash `scripts/bundle-slint-app.sh` pour la création d'un `.app` sans `tauri-bundler`.

## Mesures Actualisées
1. **Bundle size** : ~9.4 MB (contre ~30-40 MB pour la version Tauri).
2. **RSS idle (après 30s)** : ~89 MB (contre > 200 MB pour Tauri + WKWebView).
3. **Temps de compilation (Release)** : Seule la couche Rust/Slint compile, plus aucun temps de build frontend JS/Vite.

## Décision Accessibilité (VoiceOver)
**Accepté (avec mitigation)** :
Slint intègre `accesskit` nativement. Combiné au **menu AppKit natif**, les raccourcis système (Copier/Coller/Quitter) et la navigation du menu sont 100% fonctionnels et accessibles avec VoiceOver.

## Motif d'intégration Rust -> Slint (Remplacement Specta)
Au lieu des bindings TypeScript générés par Specta et du transport IPC JSON webview :
- **État (Rust -> Slint)** : Modification directe des `property` Slint via les setters générés en Rust (`ui.set_text_state(...)`).
- **Callbacks (Slint -> Rust)** : Callbacks synchrones via `ui.on_button_clicked(move || { ... })`.
- Pas de couche de sérialisation JSON ou d'IPC inter-processus.

## Conclusion
**Go**. La coquille Slint autonome sans Tauri est validée.
