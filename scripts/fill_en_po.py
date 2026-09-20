import re

translations = {
    "Modèle non téléchargé - ouvrez Réglages pour le télécharger.": "Model not downloaded - open Settings to download.",
    "Le modèle est en cours de préparation. Réessayez lorsqu’il est prêt.": "Model is preparing. Please try again when ready.",
    "Le modèle est en erreur. Réessayez depuis Réglages.": "Model error. Please retry from Settings.",
    "La réunion a quitté son état de finalisation de façon inattendue.": "The meeting left its finalization state unexpectedly.",
    "La finalisation de la réunion a dépassé 30 secondes.": "Meeting finalization timed out after 30 seconds.",
    "Export terminé vers {}": "Export completed to {}",
    "Le modèle est en erreur.": "The model is in an error state.",
}

with open("src-tauri/souffle-slint/lang/en/LC_MESSAGES/souffle-slint.po", "r", encoding="utf-8") as f:
    content = f.read()

for fr, en in translations.items():
    pattern = rf'msgid "{fr}"\nmsgstr ""'
    replacement = f'msgid "{fr}"\nmsgstr "{en}"'
    content = re.sub(pattern, replacement, content)

# Check for empty msgstrs
for match in re.finditer(r'msgid "(.*?)"\nmsgstr ""', content):
    if match.group(1):
        print(f"Empty translation found: {match.group(1)}")

with open("src-tauri/souffle-slint/lang/en/LC_MESSAGES/souffle-slint.po", "w", encoding="utf-8") as f:
    f.write(content)
