# SOU-166 native dictionary learning verification

Verified on 2026-10-05 in an isolated native Skia window using
`cargo run --manifest-path app/Cargo.toml -p souffle-slint --example live_dictionary_edit`.
`SOU166_LOCALE=en` selects English. The fixture contains synthetic text and a temporary SQLite
database. The live editor and accept/dismiss handlers are the production callbacks; no user
profile, microphone or running Nightly instance was changed.

- Accessibility exposed the paragraph's Correct button, the editor's labelled text area,
  Save/Cancel, Settings, Accept and Dismiss. Those actual AX controls were used to drive the flow.
- FR: corrected `Kubernetis` to `Kubernetes` in the native paragraph editor. The live transcript
  updated after the backend committed. SQLite held one pending suggestion and zero accepted terms.
- FR: Accept removed the suggestion and produced one canonical `Kubernetes` dictionary entry with
  alias `Kubernetis`. Both the actual Settings UI and SQLite state were inspected.
- EN: repeated the correction with translated native controls, then Dismiss. SQLite and the UI
  showed no pending suggestion and no accepted entry.
- Both fixture processes exited cleanly through Cmd+Q. The pre-existing user Nightly was preserved.

Screenshots: [FR editor](editor-fr.png), [FR pending](queue-fr.png),
[FR accepted](accepted-fr.png), [EN editor](editor-en.png), [EN dismissed](dismissed-en.png).

The full current Contracts gate passed: sidecar test/debug build, fmt, workspace/all-targets
clippy with warnings denied, workspace test compilation and watchdog run, translation extractor
1.17.1 and FR/EN catalogue check. CodeQL was explicitly deferred by the user to release preparation.

Automated coverage additionally verifies legacy mode migration, cross-source deduplication and
restart persistence, persistent dismissal, transactional/idempotent acceptance, rejection of whole
edits with more than eight admissible pairs, all three learning modes, immediate session/manual
corrections, stale editor session/text rejection, stable segment indices after sorting/eviction,
preview isolation and delayed dictionary snapshots. Native audio capture/post-paste into a separate
application was not exercised by this isolated UI fixture; existing AX target/generation guards and
post-paste collection are covered by the workspace tests.

Code review follow-up: [English suggestion queue](queue-en-accessibility.png)
shows the mode consistently named “Suggestions”. Native Accessibility inspection
confirmed separate names for each row's actions, including
`Accept Kubernetis → Kubernetes` / `Dismiss Kubernetis → Kubernetes` and a second
synthetic correction. The visible button text remains Accept / Dismiss. The
isolated process was closed after inspection; no user profile was touched.
