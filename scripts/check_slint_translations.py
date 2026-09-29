"""Reject missing, fuzzy and placeholder Slint catalogue entries."""

import ast
import re
import sys
from pathlib import Path


# These terms, names and symbols genuinely have the same spelling in both
# languages. An unchanged French sentence in the English catalogue is not a
# translation; make any new exception an explicit review decision.
UNCHANGED_ENGLISH = {
    "100% local", "AUDIO", "Audio", "Claude Code", "DIAGNOSTICS",
    "English", "EXPANSION", "INTERFACE", "Interface", "MICROPHONE",
    "NOTES", "Notes", "PERMISSIONS", "Soufflé", "Transcription", "×", "✕",
    # Symbols, acronyms and placeholder-only templates: identical spelling
    # in both languages is correct, not a missed translation.
    "USB", "Bluetooth", "⏸", "▶", "✓", "•",
    # Export file formats (MeetingDetail's Export menu): format names, not
    # prose - the same words in both languages.
    "Markdown (.md)", "JSON (.json)", "Audio (.ogg)",
    " {}", " / {}", "• {}", "→ {}", "Version {}",
}


def check_catalogue(path: Path, language: str) -> list[str]:
    errors: list[str] = []
    entry: dict[str, str] = {}
    flags: set[str] = set()
    current_field = ""

    def finish() -> None:
        if not entry or not entry.get("msgid"):
            return
        label = f'{entry.get("msgctxt", "")}: {entry["msgid"]}'
        if "fuzzy" in flags:
            errors.append(f"fuzzy: {label}")
        translation = entry.get("msgstr", "")
        if not translation.strip():
            errors.append(f"untranslated: {label}")
        if re.search(r"\[(?:EN|FR|TODO)\]", translation, re.IGNORECASE):
            errors.append(f"placeholder translation: {label}")
        if language == "en" and entry["msgid"] == translation and translation not in UNCHANGED_ENGLISH:
            errors.append(f"French text copied into English catalogue: {label}")
        if entry["msgid"].count("{}") != translation.count("{}"):
            errors.append(f"placeholder count differs: {label}")

    for line in path.read_text(encoding="utf-8").splitlines() + [""]:
        if not line:
            finish()
            entry = {}
            flags = set()
            current_field = ""
        elif line.startswith("#~"):
            continue
        elif line.startswith("#,"):
            flags.update(part.strip() for part in line[2:].split(","))
        elif line.startswith(("msgctxt ", "msgid ", "msgstr ")):
            current_field, literal = line.split(" ", 1)
            entry[current_field] = ast.literal_eval(literal)
        elif line.startswith('"') and current_field:
            entry[current_field] += ast.literal_eval(line)

    return errors


if __name__ == "__main__":
    language, filename = sys.argv[1:]
    failures = check_catalogue(Path(filename), language)
    if failures:
        for failure in failures:
            print(f"{language}: {failure}", file=sys.stderr)
        sys.exit(1)
    print(f"{language}: all extracted messages translated")
