#!/usr/bin/env bash
set -e

cd "$(dirname "$0")/../src-tauri/souffle-slint"

slint-tr-extractor -o lang/messages.pot $(find ui -name "*.slint")

MISSING=0
for lang in en fr; do
    po_file="lang/$lang/LC_MESSAGES/souffle-slint.po"
    msgmerge -U "$po_file" lang/messages.pot --quiet --backup=none
    
    # Check for empty msgstr, except for the header (which has empty msgid "")
    empty_count=$(awk 'BEGIN{RS=""; FS="\n"} $0 ~ /^msgid "[^"]+"/ && $0 ~ /msgstr ""/ {print}' "$po_file" | wc -l)
    if [ "$empty_count" -gt 0 ]; then
        echo "Error: $empty_count untranslated strings in $po_file"
        MISSING=1
    fi
done

if [ "$MISSING" -eq 1 ]; then
    echo "Translations check failed!"
    exit 1
fi
echo "Translations check passed."
