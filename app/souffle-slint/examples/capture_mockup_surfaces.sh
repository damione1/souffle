#!/usr/bin/env bash
# Captures the fixture surfaces matching the design boards (FR, light unless noted).
# Fixed data only: no bootstrap, database, network or user preferences.
#
#   examples/capture_mockup_surfaces.sh                  # every capture
#   examples/capture_mockup_surfaces.sh Reglages-Audio   # one board, or one capture name
#
# OUT=<dir> overrides the output directory, SKIP_BUILD=1 reuses the last build.
set -euo pipefail

cd "$(dirname "$0")/../.."
out="${OUT:-/tmp/souffle-parity/app}"
filter="${1:-}"
mkdir -p "$out"
if [[ "${SKIP_BUILD:-0}" != 1 ]]; then
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo build -p souffle-slint --example typography_qa
fi
bin="${CARGO_TARGET_DIR:-target}/debug/examples/typography_qa"

# artboard surface dark width height
# A "full" capture shows what falls below the board height: the app page is
# currently taller than the drawing.
captures="
Accueil-Clair home 0 1040 1180
Accueil-Sombre home 1 1040 1180
Accueil-Premier-Lancement empty 0 1040 700
Accueil-Alertes alerts 0 1040 1060
Accueil-Recherche-Vide search-empty 0 1040 660
Onboarding-1-Permissions onboarding-permissions 0 1040 860
Onboarding-2-Micro onboarding-microphone 0 1040 860
Onboarding-3-Modele onboarding-model 0 1040 860
Onboarding-4-Raccourci onboarding-shortcut 0 1040 860
Enregistrement-Dictee dictation 0 1040 720
Enregistrement-Reunion meeting 0 1040 1010
Reunion-Lecture detail 0 1040 1500
Reunion-Transcription detail-transcript 0 1040 1300
Reunion-Etats detail-no-summary 0 1040 1180
Reunion-Etats detail-no-provider 0 1040 1180
Reunion-Etats detail-loading 0 1040 1180
Reunion-Etats detail-error 0 1040 1180
Reunion-Etats detail-empty 0 1040 1180
Reunion-Etats detail-no-audio 0 1040 1180
Reglages-Transcription settings-transcription 0 1040 1700
Reglages-Transcription settings-transcription-long 0 1040 1900 long
Reglages-Transcription settings-transcription-long-scrolled 0 1040 1900 long-scrolled
Reglages-IA settings-ai 0 1040 1720
Reglages-IA settings-ai 0 1040 2900 full
Reglages-IA settings-ai-ollama 0 1040 1720 ollama
Reglages-Audio settings-audio 0 1040 1900
Reglages-Audio settings-audio 0 1040 2400 full
Reglages-Interface settings-interface 0 1040 1360
Reglages-Interface settings-interface 0 1040 1800 full
Reglages-Reunions settings-meetings 0 1040 760
Reglages-Reunions settings-meetings-denied 0 1040 860 denied
Reglages-Systeme settings-system 0 1040 2500
Reglages-Systeme settings-system 0 1040 3000 full
Dialogues permissions 0 1040 860
Dialogues whats-new 0 1040 860
Dialogues update 0 1040 860
Dialogues update-downloading 0 1040 860
Dialogues update-failed 0 1040 860
"

count=0
failed=0
while read -r board surface dark width height variant; do
    [[ -n "$board" ]] || continue
    name="$board"
    case "$board" in
        Reunion-Etats | Dialogues) name="$board--$surface" ;;
    esac
    [[ -z "$variant" ]] || name="$name--$variant"
    [[ -z "$filter" || "$filter" == "$board" || "$filter" == "$name" ]] || continue
    count=$((count + 1))
    png="$out/$name.png"
    rm -f "$png"
    # Scale 1 matches the board renders.
    if ! SLINT_SCALE_FACTOR=1 SOU276_MOCKUP=1 SOU276_LANG=fr SOU276_SURFACE="$surface" \
        SOU276_DARK="$dark" SOU276_WIDTH="$width" SOU276_HEIGHT="$height" \
        SOU276_SNAPSHOT="$png" "$bin" </dev/null >"$out/$name.log" 2>&1; then
        echo "FAIL $name: fixture exited with an error, see $out/$name.log" >&2
        failed=1
        continue
    fi
    size="$(sips -g pixelWidth -g pixelHeight "$png" 2>/dev/null |
        awk '/pixelWidth/ { w = $2 } /pixelHeight/ { h = $2 } END { print w "x" h }')"
    if [[ "$size" != "${width}x${height}" ]]; then
        echo "FAIL $name: got $size, expected ${width}x${height}" >&2
        failed=1
        continue
    fi
    rm -f "$out/$name.log"
    echo "ok $name ($size)"
done <<<"$captures"

if [[ "$count" == 0 ]]; then
    echo "no capture matches '$filter'" >&2
    exit 1
fi
exit "$failed"
