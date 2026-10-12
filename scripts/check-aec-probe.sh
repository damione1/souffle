#!/bin/sh
# Explicit, interactive hardware QA only. Never called by builds or tests.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    echo "Usage: $0 INPUT_UID [NEW_QA_DIRECTORY]" >&2
    echo 'Find INPUT_UID with the signed probe devices command.' >&2
    exit 2
fi
probe="$root/app/target/aec-probe/Souffle AEC Probe.app/Contents/MacOS/souffle-aec-probe"
[ -x "$probe" ] || { echo 'Build the probe with ./scripts/build-aec-probe.sh first.' >&2; exit 1; }
input_uid=$1
qa=${2:-"$HOME/Souffle-QA/SOU-278-$(date +%Y%m%d-%H%M%S)"}
umask 077
mkdir -p "$(dirname -- "$qa")"
mkdir "$qa" # Refuse overwrite.
cat > "$qa/notes.txt" <<'NOTES'
Microphone mode (choose Standard for the probe):
Browser / video URL and exact starting position:
Speaker volume / webcam position / distance from mouth:
Remote only: raw / FDAF / Apple residual remote voice:
Local only: raw / FDAF / Apple preservation of local voice:
Double talk: raw / FDAF / Apple remote residual and local voice cuts:
Other apps recording / device or volume changes / permission prompts:
NOTES
printf 'Dossier privé : %s\nMicrophone UID : %s\n' "$qa" "$input_uid"
printf '%s\n' 'Choisis Standard dans les contrôles micro du probe. Garde le volume, la webcam et ta position fixes.'
printf '%s\n' 'Chaque prise dure 30 s après le message RECORDING. Entrée pour commencer ; q pour arrêter.'
for scene in remote local double; do
    case "$scene" in
        remote) instruction='VIDÉO SEULE : prépare le même extrait, haut-parleurs audibles, ne parle pas.' ;;
        local) instruction='VOIX SEULE : mets la vidéo en pause, parle normalement, marque une pause puis reprends.' ;;
        double) instruction='LES DEUX : prépare le même extrait et parle en même temps, avec une pause puis une reprise.' ;;
    esac
    for mode in baseline apple; do
        printf '\n%s\nMode : %s. Remets la vidéo au même point avant chaque prise. [Entrée/q] ' "$instruction" "$mode"
        IFS= read -r answer
        case "$answer" in q|Q) exit 0 ;; esac
        "$probe" check "$qa/$scene-$mode" "$mode" "$scene" --input-uid "$input_uid" \
            --notes 'SOU-278 interactive listening check; observations in parent notes.txt'
    done
done
printf '\nTerminé : %s\n' "$qa"
printf '%s\n' 'Écoute raw.wav et fdaf.wav dans chaque dossier baseline, puis apple.wav dans le dossier Apple correspondant.'
printf '%s\n' 'Note les observations dans notes.txt. Garde manifest.json et runtime.log, même en cas de refus matériel.'
