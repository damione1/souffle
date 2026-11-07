# SOU-281: Apple microphone QA with real audio

The experimental Nightly is ready for an operator to test when the signed bundle
and launch checks below pass. Acoustic effectiveness, the 27-variant matrix and
meeting-app coexistence still require Damien's physical trials. A successful
build, MCP interaction or Apple activation log does not establish those results.

## Build the QA app

Run from this worktree with the project's existing Developer ID identity and
shared Cargo target. Building does not launch the app or record audio.

```sh
export CARGO_TARGET_DIR=/Users/damien/Library/Caches/codex-build/souffle/app/target
export CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only
MACOSX_DEPLOYMENT_TARGET=13.0 ./scripts/bundle-macos.sh --debug --nightly --native-aec --slint-mcp
app="$PWD/app/target/debug/bundle/macos-native-aec-mcp/Soufflé Nightly.app"
codesign --verify --deep --strict "$app"
codesign -dv --verbose=2 "$app" 2>&1
/usr/libexec/PlistBuddy -c 'Print :SouffleNativeAEC' "$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c 'Print :SouffleSlintMCP' "$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c 'Print :SouffleSourceCommit' "$app/Contents/Info.plist"
```

Both markers must be `true`, the source commit must match the delivery, and the
signature must identify the expected Developer ID. The general bundle script can
build unsigned if the identity is missing: that result is **not** a signed QA
delivery. This variant has the usual Nightly identity/preferences and a separate
bundle path. Quit another running Nightly yourself before launching it; retain
its data and permissions.

`--slint-mcp` is allowed only with `--debug --nightly` and cannot produce an
updater artifact. It enables Slint's dependency feature at build time without
changing Cargo defaults. Omitting the flag preserves the ordinary build paths.
The native microphone is opt-in; dictation continues to use CPAL.

## First real-audio check in the app

Create a private evidence directory, then launch the signed executable. Audio
starts only when you start a meeting in the app.

```sh
umask 077
qa="$HOME/Souffle-QA/SOU-281-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$HOME/Souffle-QA"
mkdir "$qa"
SLINT_MCP_PORT=9315 "$app/Contents/MacOS/souffle" > "$qa/app.log" 2>&1
```

1. Select the webcam explicitly in Soufflé's input settings. Keep its position,
   mouth distance, speaker volume and output fixed. Choose Standard in macOS
   microphone controls when available. Record the OS, selected device, browser
   version, fixture starting position and actual scripts in private notes.
2. Using the same browser passage, make three separate meetings of 30–40 seconds:
   remote speech only, local speech only, then both together. Include a fixed
   pause and resumption. Record local words actually spoken; an ASR transcript
   is not a reference. Keep the original meetings and logs, including failures.
3. Repeat with the same installed Whisper weights and then the same installed
   Parakeet weights, recording catalogue model/backend IDs. Listen for remote
   speech leaking into Me and local words being cut. Note these separately.
4. In the logs require `Apple meeting microphone requested=... readbacks=...`
   with the chosen input and successful start, then
   `Meeting backend=Apple VoiceProcessingIO, FDAF=disabled`. Check AGC is
   disabled, negotiated formats, callback losses/errors and route rebuilds.
   An intent log or visible Me/Them text alone is insufficient.
5. Quit with ⌘Q after stopping the meeting. Record any failure to stop or quit.

If the explicitly chosen input cannot start, retain the error as a failed trial.
Do not substitute another input or treat a fallback as successful Apple capture.
These app checks establish operator observations; they do not populate the
formal probe matrix below. Keep WAVs, transcripts and hardware UIDs outside Git.

## Retest startup after an I/O configuration change

The first operator QA exposed a native engine stopping itself while Apple's
hardware channel layout changed during startup. The mono client format and
selected devices were correct. The corrected Nightly observes that transition,
allows one complete reread if notification crosses a readback, and permits at
most one restart of the unchanged graph. Cancellation and the original startup
deadline still apply; other device/format/processing failures remain errors.

Before continuing the acoustic trials, use the same built-in input and speaker
route that exposed the failure. Perform **meeting → stop → dictation → stop →
meeting** two or three times, waiting for each stop to complete. Start recordings
yourself; automated UI inspection must not activate capture. Check that the
meetings start, receive your speech, preserve their tail, and stop normally.

Retain the new run's logs separately from earlier runs. Require the expected
device/format readbacks, `running=true`, both voice-processing nodes enabled,
AGC/bypass off, and normal callbacks. A recovered startup must have at most two
start attempts; an ordinary startup needs no retry. If it still fails, retain
the precise reason and attempt/notification snapshots instead of substituting
another microphone or backend. A successful restart does not establish echo
suppression or voice preservation; continue the listening checks above.

The developer probe does not use the application's owner-controlled restart
policy. Keep failed formal probe trials as evidence; these app regression checks
do not replace or populate the formal comparison matrix.

## Inspect the app without recording

The optional server listens on localhost only when `SLINT_MCP_PORT` is set at
launch. Choose a free port and stop this QA app after inspection. The endpoint
can inspect the UI and invoke actions; leave recording actions to the operator.
This is Slint's UI server, separate from Soufflé's transcription MCP sidecar.

```sh
curl --fail --silent --show-error -X POST http://127.0.0.1:9315/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'
```

Use the returned tools to list windows, inspect the element tree, take a
screenshot and exercise settings/navigation. Do not click a recording control
as part of automated UI verification. The app continues to use Skia/Metal.

## Formal comparison: remaining operator evidence

Build the isolated, signed probe with `./scripts/build-aec-probe.sh`. Set
`probe="$PWD/app/target/aec-probe/Souffle AEC Probe.app/Contents/MacOS/souffle-aec-probe"`.
Its `devices` command lists current routes without capture. The full system tap
requires macOS 14.4 or newer; targeting macOS 13 does not validate that OS.

The formal `capture` command uses the current default input/output; it does not
accept `--input-uid`. Confirm the current route already matches the intended
hardware and verify the manifest. Do not change global devices as an automated
workaround. For a pinned webcam listening check use
`./scripts/check-aec-probe.sh '<webcam UID>'` instead; those auxiliary captures
cannot enter the formal matrix.

Initialize a **new** private directory with `"$probe" init "$formal"` after setting
`formal="$qa/formal"`. Complete its `protocol.json` before recording:

- One fixed mono 30-second remote WAV with a SHA256 and a manually checked
  audible reference. The repository's `ecorp-q4-intro.wav` is about 79 seconds;
  prepare a derivative outside Git before use. The positive-control loader
  rejects fixtures over 60 seconds.
- A distinct local script actually read, exact pause/resume schedule, position,
  positive mouth distance, volume, browser/version and Standard mode assertion.
- Existing catalogue IDs and SHA256 weights manifests for Whisper and Parakeet.
  Use `"$app/Contents/MacOS/souffle" --list-models --json` for the catalogue.
  Keep the weight manifests privately and verify unchanged weights before ASR.

For repetition `1`, run the six commands below individually after preparing the
scene. Wait for `RECORDING` after the countdown; record actual playback offsets
and reading deviations in `--notes`. Repeat the six with `2`, then `3`.

```sh
"$probe" capture "$formal" baseline remote 1 --notes 'actual playback offset; no local speech'
"$probe" capture "$formal" baseline local 1 --notes 'actual reading deviations; browser paused'
"$probe" capture "$formal" baseline double 1 --notes 'actual playback offset and reading deviations'
"$probe" capture "$formal" apple remote 1 --notes 'actual playback offset; no local speech'
"$probe" capture "$formal" apple local 1 --notes 'actual reading deviations; browser paused'
"$probe" capture "$formal" apple double 1 --notes 'actual playback offset and reading deviations'
```

This produces **18 physical captures, 27 analysis variants**: nine paired raw/FDAF
outputs from identical baseline PCM, plus nine separate Apple captures. Preserve
invalid/incomplete attempts. Before retrying a name, move the failed directory
to a unique `archive-*` directory. A changed protocol requires a new series.

For the positive control, stop browser playback: only the probe plays the fixture.
Its separate `-control` folder stays outside the passive comparison.

```sh
"$probe" capture "$formal" apple remote 1 --positive-control --notes 'native source control; browser stopped'
```

Transcribe every completed trial first with Whisper, then Parakeet. The signed
QA app has the existing headless ASR CLI. No download is performed by the wrapper.

```sh
for engine in whisper parakeet; do
  for mode in baseline apple; do
    for scene in remote local double; do
      for rep in 1 2 3; do
        "$probe" transcribe "$formal/$mode-$scene-$rep" \
          --binary "$app/Contents/MacOS/souffle" --engine "$engine" || break 4
      done
    done
  done
done
"$probe" report "$formal"
```

Transcribe the `apple-remote-1-control` folder separately with both engines.
Hashes bind protocol, WAVs, ASR results and successful provenance. Normalize to
lowercase, remove punctuation, split on spaces/whitespace and retain accents.
Report each trial's local WER/deletions and remote overlap proxy, then median and
range by engine/scene/variant. Shared reference words are ambiguous; listening
must identify true remote duplicates and missing local words. Double-talk WER
includes remote insertions. `comparison.json` lists missing trials explicitly;
`complete:true` describes matrix coverage, not acoustic acceptance.

## Coexistence, routes and final report

In a separate private protocol directory use auxiliary 60-second Apple captures:
`"$probe" capture "$coexist" apple double 1 --seconds 60 --notes 'app name/version; meeting app started first'`,
then repetition `2` with the probe started first. Verify for one minute per order
that the **other application/remote endpoint actually receives your voice**.
Record before/after rates and volume, losses/errors and audible ducking. Exercise
headphones, mute/unmute and device changes in repetition `3`, with exact timings
in notes. Preserve route-invalid trials as auxiliary evidence; callbacks do not
prove voice transmission. Never reset TCC or hardware rates to hide a failure.

The eventual authorized report must distinguish browser AEC from the native
positive control, echo suppression from local voice preservation, and API state
from acoustic results. Include available CPU/RSS/delivery-age diagnostics with
their limits: dev build, process high-water RSS, offline FDAF cost separate from
live Apple, and tap/mic alignment without hardware timestamp synchronization.

| SOU-281 evidence | Before Damien's hardware QA |
|---|---|
| AC1: 18 captures / 27 variants, references and deviations | Pending; missing takes remain named |
| AC2: both engines, per-trial metrics and dispersion | Pending captured audio and pinned weights |
| AC3: browser case vs native positive control, graph logs | Pending runtime capture/readbacks |
| AC4: voice received in other app, both orders and routes | Pending operator observations |
| AC5: measured conclusion with cost/latency limits | Pending measurements; no default-adoption threshold |
| AC6: production capture/settings unchanged | QA packaging only; native path remains opt-in |

Publish only approved metadata, metrics and listening conclusions. Keep private
audio, references, ASR text and device identifiers outside the PR. macOS versions
not physically tested remain named limitations.
