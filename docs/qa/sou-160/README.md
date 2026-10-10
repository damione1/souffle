# SOU-160 native Settings verification — 2026-10-10

This verification follows the final displayed-template identity correction. Built the actual
worktree binary with the shared Cargo cache and
`./scripts/bundle-macos.sh --debug --nightly`; the local bundle passed Developer
ID signature verification. Unique `SOUFFLE_APP_IDENTIFIER` profiles isolated
French and English data from production and normal Nightly. Apple Speech avoided
a model-weights download. Screen capture and Accessibility access were verified
through the platform APIs. One application process ran at a time.

macOS Accessibility supplied the window bounds: `1200,63,1040,1280`.
`screencapture -x -R` captured each full-window image below, and every committed
image was opened using the `souffle-ui-screenshot` workflow.

The French/English switch instructions, French restart-to-Code result and
English global fallback were recaptured and viewed on the final
binary, including the displayed-template identity correction. Both profiles exited
with status 0 through the native Quit menu.
The other five images below document the round-two binary, commit `5c30fd8a`;
The later corrections change canonical projection and prompt identity without
changing layout.

Actual native interactions and durable results:

1. Pressed Tester with an ordinary mouse click while Soufflé was frontmost.
   The [French instruction](fr-switch-instruction.png) and
   [English instruction](en-switch-instruction.png) told the user to switch
   applications before capture in three seconds. Activated Terminal during
   that interval; it remained frontmost until the interval ended. Restoring
   Soufflé showed the captured Terminal name. No app name was injected and no
   cached Accessibility press was used for Tester.
2. With both `Terminal → Chat` and `Term → Code` enabled, moved the Terminal
   rule first. [Chat won](fr-terminal-chat.png), although both patterns matched.
   Moved it down: [Code won](fr-terminal-code.png). SQLite persisted the same
   IDs and template references in the changed order.
3. Started Tester and changed a rule's switch during its waiting interval.
   [The cancelled test](fr-cancelled-test.png) remained hidden after more than
   three seconds; canonical rule rows still updated. A new test with the
   first rule disabled selected the remaining Chat rule.
4. Disabled both rules. The ordinary test returned the global Cleanup template
   with the [French no-match diagnostic](fr-global-fallback.png). The English
   profile's disabled suggestions also produced the
   [global fallback](en-global-fallback.png).
5. From the new-rule input, three Tab presses reached the first rule's switch;
   Space enabled `Term`. Quit through the native menu exited with status 0.
   Relaunching the profile preserved IDs, order, activation and targets byte
   for byte. [The restarted app](fr-restart-code.png) resolved Terminal to Code
   through the same ordinary click/switch workflow; its result also remained
   visible after an additional two seconds.
6. The actual [English template dropdown](en-template-menu.png) displayed
   Default, Cleanup, Professional email, Bullets, No fillers, Chat message and
   Code editor, without raw translation tokens. Both language profiles exited
   normally through the native Quit menu.

The full local gate passed: sidecar resolution/build, workspace formatting,
strict Clippy, test compilation, watchdog execution (1,545 passed, 20 explicit
opt-in/manual ignores, zero failures), extractor installation and EN/FR
translation checks. Two main-thread keyboard/paste harnesses also completed.
The 27 editor tests include blocked-worker A → B → A, rejected-save retry,
mock-clock delayed capture, frozen capture, supersession, actual global/enable
callback cancellation and close/reopen regressions. Four additional blocked-worker
tests exercise global-template and enable changes followed by a rule save or
Tester, complete canonical control projection, and shared prompt draft preservation
with edits persisted to the displayed template. A further Intermediate-response
regression advances the cache to Email while Cleanup remains displayed; the real
prompt callback edits Cleanup alone, preserving Email's prompt. CodeQL was skipped by
explicit user instruction for this batch.

Physical microphone dictation in chat/mail/code and a real focus change during
an audio Stop were not run. Production polish HTTP pipeline tests cover shared
prompt updates, the supplied stop snapshot, invalid-target global fallback and
disabled polish; existing snippet/paste tests remain green. These captures prove
Settings behavior and persistence, not ASR or LLM quality.
