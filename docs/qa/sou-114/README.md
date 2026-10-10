# SOU-114 native verification

Captured and viewed on 2026-10-10 from source commit `9eab704d`, after the pending-start cancellation correction and its complete local gate. The following commit adds only these QA documents and images; it does not change the built code.

## Build and isolation

`CARGO_TARGET_DIR=/Users/damien/Projects/souffle/app/target CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 ./scripts/bundle-macos.sh --debug --nightly` produced a native Slint/Skia bundle signed with Developer ID Application: Damien Goehrig (`X6H966RSDB`). Codesign verification passed. FR and EN used separate fresh `SOUFFLE_APP_IDENTIFIER` profiles, Apple Speech, light theme and existing macOS permissions. Their database initially had retention Off and no diagnostic preference. No production recordings or settings were changed. Screenshots are unedited window captures at 1040 × 1280.

## Native control observations

| Image | Observed behavior |
| --- | --- |
| [fr-default-disabled.png](fr-default-disabled.png) | Fresh FR profile: diagnostic switch unchecked and disabled; explanation directs the user to audio retention in Data. A physical click leaves the preference absent and retention Off. |
| [fr-enabled-after-restart.png](fr-enabled-after-restart.png) | Physical selection of seven-day retention and switch click persist `meeting_audio_diagnostic=true` and `meeting_audio_retention="keep_7d"`. Native Quit exits with code 0. Restart shows the switch still checked and the complete lane/cost/cap explanation. |
| [en-default-disabled.png](en-default-disabled.png) | Fresh EN profile: translated advanced Audio label and disabled default switch, with the translated retention explanation. |
| [en-enabled.png](en-enabled.png) | Physical retention selection and switch click persist the same values in the separate EN database. The label explains microphone left, system audio right, 1382 MB/hour and the 60-minute cap. Native Quit exits with code 0. |

Settings tabs were navigated through their actual accessibility controls. Retention popup items and diagnostic switches were clicked with CGEvent mouse events (`cliclick`), using captured screen coordinates. Persistence was read directly from each isolated SQLite database. No synthetic Rust callback was used.

## Hardware capture and replay smoke

On the same `9eab704d` build, the FR profile started a meeting with a physical click on the visible home action. A 19.47-second generated speech file was played with `afplay` through the system output; the meeting was stopped through its native button. Runtime logs confirmed a 48 kHz system tap, AEC engaged and a healthy system-audio leg with 1,010,688 samples.

| Artifact | `ffprobe` result |
| --- | --- |
| `0.pre-aec.wav` | `pcm_f32le`, 48,000 Hz, two channels, 21.109333 seconds |
| `0.ogg` | Opus, 48,000 Hz, two channels, 21.120000 seconds |
| Separated raw left / right | Two float32 monos, each 48,000 Hz and 21.109333 seconds |

`ffmpeg` whole-file `astats` showed a silent microphone lane and a nonzero right system lane (peak −2.275891 dB, RMS −16.171465 dB). This verifies the actual system capture and separate artifact path. It does not supply near-end voice ground truth.

The shipped `--aec-replay` entry point exited successfully and reported 1,013,248 captured frames, 96,000 measured frames, unavailable estimated delay and zero raw/compensated mic-output ratios. Those values are expected with a silent microphone lane. The JSON includes the attenuation-proxy and pipeline-lag definition; this smoke makes no echo-removal quality claim.

Commands used on the private isolated artifact:

```sh
ffprobe -v error -show_entries stream=codec_name,sample_rate,channels -show_entries format=duration -of json "$diagnostic"
ffmpeg -v error -i "$diagnostic" -af 'pan=mono|c0=c0' -c:a pcm_f32le raw-mic.wav
ffmpeg -v error -i "$diagnostic" -af 'pan=mono|c0=c1' -c:a pcm_f32le raw-tap.wav
"$bundle/Contents/MacOS/souffle" --aec-replay "$diagnostic"
```

Raw audio, transcripts, databases and runtime logs remain outside the repository. A prior 49-second smoke on `04d3159d` was superseded by this capture after correction and is not presented as proof of the final lifecycle.

## Automated evidence and limits

The final eight-command gate passed: sidecar contract/build, fmt, strict workspace/all-target Clippy, workspace compilation and watchdog tests, extractor 1.17.1, FR/EN translation extraction. Tests: 1,551 passed, 21 existing ignores, plus the two main-thread harnesses. The first implementation regression and the later Stop-before-start-failure regression were both observed red before their production fixes. Independent review reran the 14 diagnostic and 17 transcription command tests.

Actual mixer/encoder tests prove byte-identical primary mono/stereo Ogg and unchanged engine-input samples with AEC active. Fault tests cover writer failure, overflow, contention, disk headroom, exact aligned cap prefixes, delayed cancellation/retry and Stop before engine readiness. Captured synthetic replay covers more than eight seconds and exactly matches direct metrics with a nonzero microphone lane. Retention excludes newer diagnostic timestamps and purges orphan partials.

The controlled two-minute loudspeaker/near-end double-talk protocol remains unrun. This smoke does not validate acoustic echo quality or an actual near-end transcription. Lossless WAV costs about 1.38 GB/hour (806 MB/35 minutes), is capped at the first 60 minutes and can stop earlier for low space. The independent writer's 256 MiB reserve is not a guarantee against concurrent external disk use. Retention still sweeps at startup. CodeQL was explicitly waived for this ticket batch.
