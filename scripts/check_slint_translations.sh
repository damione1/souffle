#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root/app/souffle-slint"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

find ui -name '*.slint' -print0 | xargs -0 slint-tr-extractor -o "$tmp/messages.pot"

for lang in en fr; do
    po="lang/$lang/LC_MESSAGES/souffle-slint.po"
    merged="$tmp/$lang.po"
    # A check must not rewrite catalogues or accept fuzzy guesses when a
    # source string changes.
    msgmerge --quiet --no-fuzzy-matching -o "$merged" "$po" "$tmp/messages.pot"
    msgfmt --check-format -o /dev/null "$merged"
    python3 "$root/scripts/check_slint_translations.py" "$lang" "$merged"
done

echo 'Translations check passed.'
