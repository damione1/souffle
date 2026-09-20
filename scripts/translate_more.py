import os
import re

# We will just write a simple regex replacement to match @tr("French string") and replace with English ones.
# Actually, it's easier to just translate the whole file directly.

translations = {
    "Audio système": "System Audio",
    "Accessibilité": "Accessibility",
    "Nécessaire pour la dictée et les réunions.": "Required for dictation and meetings.",
    "Capture l'autre côté d'un appel dans les réunions.": "Captures the other side of a call in meetings.",
    "Nécessaire pour le collage automatique et certains raccourcis.": "Required for auto-paste and some shortcuts.",
    "Ouvrir les Réglages": "Open Settings",
    "Refusé. Ouvrez Réglages Système > Confidentialité et sécurité > Accessibilité.": "Denied. Open System Settings > Privacy & Security > Accessibility.",
    "Refusé. Ouvrez Réglages Système pour l'autoriser.": "Denied. Open System Settings to allow it.",
    "Aucun microphone détecté.": "No microphone detected.",
    "Accordé": "Granted",
    "Réparation effectuée, en attente de confirmation macOS.": "Repair completed, waiting for macOS confirmation.",
    "Un problème persiste ? Réparez l'entrée Accessibilité.": "Still having issues? Repair the Accessibility entry.",
    "Réparer": "Repair",
    "Rechercher les transcriptions et réunions": "Search transcriptions and meetings",
    "Lancer à l'ouverture de session": "Launch at login",
    "Ouvrir Soufflé automatiquement au démarrage du Mac.": "Open Soufflé automatically when Mac starts.",
    "Lancer Soufflé à l'ouverture de session": "Launch Soufflé at login",
    "Modèle de transcription": "Transcription Model",
    "Choisissez un modèle : il se télécharge et se charge tout seul. Tout tourne sur ce Mac.": "Choose a model: it downloads and loads on its own. Everything runs on this Mac.",
    "Délai avant déchargement": "Unload timeout",
    "Décharger le modèle de la mémoire après une période d'inactivité.": "Unload the model from memory after a period of inactivity.",
    "Nouveautés": "What's New",
    "Sections des paramètres": "Settings sections",
    "Réunions": "Meetings",
    "Système": "System",
    "Français": "French",
    "Microphone quand le capot est fermé": "Microphone when lid is closed",
    "Utiliser ce microphone à la place quand le capot est fermé avec un écran externe connecté.": "Use this microphone instead when the lid is closed with an external display connected.",
    "Capturer l'audio système": "Capture system audio",
    "Langue de transcription (réunions)": "Transcription language (meetings)",
    "Indice pour la détection de langue, jamais forcé sur le moteur.": "Hint for language detection, never forced on the engine.",
    "Langue de transcription des réunions": "Meetings transcription language",
    "Arrêt automatique des réunions": "Auto-stop meetings",
    "Délai avant arrêt": "Delay before stop",
    "Délai avant arrêt automatique": "Delay before auto-stop",
    "Durée maximale d'une réunion": "Maximum meeting duration",
    "Détection de voix (VAD)": "Voice Activity Detection (VAD)",
    "Détection de voix": "Voice Activity Detection",
    "Suppression des hésitations": "Hesitation removal",
    "Réduction des bégaiements": "Stutter reduction",
    "Fusionner les répétitions et faux départs.": "Merge repetitions and false starts.",
    "Le modèle par défaut est utilisé pour résumer automatiquement une réunion terminée.": "The default template is used to automatically summarize a finished meeting.",
    "Modèle par défaut": "Default Template",
    "Modèle de résumé par défaut": "Default summary template",
    "Modèle de résumé à modifier": "Summary template to edit",
    "Nom du modèle de résumé": "Summary template name",
    "Modèle intégré : le prompt peut être personnalisé mais pas supprimé.": "Built-in template: the prompt can be customized but not deleted.",
    "Prompt envoyé à l'IA pour générer le résumé.": "Prompt sent to AI to generate the summary.",
    "Instruction de génération du résumé": "Summary generation instruction",
    "Instruction du modèle de résumé": "Summary template instruction",
    "Nom du nouveau modèle": "New template name",
    "Nom du nouveau modèle de résumé": "New summary template name",
    "Nettoyer la dictée avec l'IA": "Clean dictation with AI",
    "Passe le même fournisseur de résumés (Ollama ou Apple Intelligence) après transcription pour corriger ponctuation, termes techniques et autocorrections parlées, sans changer le sens.": "Runs the same summary provider (Ollama or Apple Intelligence) after transcription to fix punctuation, technical terms, and spoken autocorrects, without changing the meaning.",
    "Modèle de retouche": "Polish model",
    "Prompt envoyé à l'IA pour retoucher le texte transcrit.": "Prompt sent to AI to polish the transcribed text.",
    "Dites une phrase déclencheur au début d'une dictée pour insérer un bloc de texte. La comparaison ignore la casse et les accents ; aucune retouche IA n'est appliquée quand un extrait correspond.": "Say a trigger phrase at the start of a dictation to insert a text block. Comparison ignores case and accents; no AI polish is applied when a snippet matches.",
    "Phrase déclencheur": "Trigger phrase",
    "Texte à insérer": "Text to insert",
    "Appuyez…": "Press...",
    "Non défini": "Not set",
    "Frappe simulée": "Simulated typing",
    "Champ ciblé": "Target field",
    "Thème": "Theme",
    "Collage automatique après dictée": "Auto-paste after dictation",
    "Colle automatiquement la transcription à l'arrêt de l'enregistrement.": "Automatically pastes the transcription when recording stops.",
    "Vérifiez chaque autorisation. La réparation réinitialise uniquement l’entrée Accessibilité avant de redemander l’accès.": "Check each permission. Repair only resets the Accessibility entry before requesting access again.",
    "Terminé": "Done"
}

def translate_file(path):
    with open(path, "r", encoding="utf-8") as f:
        content = f.read()

    changed = False
    for fr, en in translations.items():
        if f'@tr("{fr}")' in content:
            content = content.replace(f'@tr("{fr}")', f'@tr("{en}")')
            changed = True

    if changed:
        with open(path, "w", encoding="utf-8") as f:
            f.write(content)

for root, _, files in os.walk("src-tauri/souffle-slint/ui"):
    for file in files:
        if file.endswith(".slint"):
            translate_file(os.path.join(root, file))
