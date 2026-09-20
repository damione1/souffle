import os
import re

translations = {
    "Rechercher transcriptions et réunions": "Search transcriptions and meetings",
    "Dictées": "Dictations",
    "Soufflé": "Soufflé",
    "Titre de la réunion": "Meeting title",
    "Ajoutez les décisions, actions et noms importants…": "Add important decisions, actions and names...",
    "Notes de la réunion": "Meeting notes",
    "Cette réunion ne contient aucun enregistrement.": "This meeting contains no recordings.",
    "Aucun audio, aucune transcription et aucun résumé.": "No audio, no transcription and no summary.",
    "Fermer les paramètres": "Close settings",
    "Paramètres": "Settings",
    "Réglages": "Settings",
    "Copié !": "Copied!",
    "Copier": "Copy",
    "Sélection": "Selection",
    "Réunion en direct": "Live meeting",
    "Dictée": "Dictation",
    "Arrêter l'enregistrement": "Stop recording",
    "Arrêter": "Stop",
    "Attention. Seul votre microphone est enregistré. L'audio des autres participants est indisponible.": "Warning. Only your microphone is recorded. Other participants' audio is unavailable.",
    "Seul votre microphone est enregistré. L’audio des autres participants est indisponible.": "Only your microphone is recorded. Other participants' audio is unavailable.",
    "À l'écoute…": "Listening...",
    "Collé automatiquement dans votre dernière app à l’arrêt": "Pasted automatically into your last app on stop",
    "Audio système demandé": "System audio requested",
    "Audio système indisponible": "System audio unavailable",
    "Notez décisions, actions, noms…": "Note decisions, actions, names...",
    "Démarrer la transcription": "Start transcription",
    "Affichez vos réunions du jour ici": "View your today's meetings here",
    "Aucun résultat": "No results",
    "Fin de l'enregistrement précédent": "End of previous recording",
    "Nouvelle session d'enregistrement": "New recording session",
    "Soufflé a besoin de ces autorisations macOS pour fonctionner.": "Soufflé needs these macOS permissions to work.",
    "Vous pourrez changer ce choix plus tard dans les Réglages.": "You can change this choice later in Settings.",
    "Téléchargé une seule fois, tout tourne ensuite hors ligne.": "Downloaded once, then everything runs offline.",
    "Utilisé pour démarrer et arrêter la dictée depuis n'importe quelle application.": "Used to start and stop dictation from any application.",
    "Terminer": "Finish",
    "Télécharger et continuer": "Download and continue",
    "Continuer": "Continue",
    "Dictée et transcription de réunion, 100% locale.": "Dictation and meeting transcription, 100% local.",
    "Le microphone peut être changé plus tard dans les Réglages.": "The microphone can be changed later in Settings.",
    "Téléchargement du modèle…": "Downloading model...",
    "Chargement du modèle…": "Loading model...",
    "Modèle prêt.": "Model ready.",
    "Le collage automatique nécessite la permission Accessibilité.": "Auto-paste requires Accessibility permission.",
    "Vérifier": "Check",
    "Les permissions peuvent être accordées plus tard.": "Permissions can be granted later.",
    "Dictée à récupérer": "Dictation to recover",
    "Le texte reste ici tant que vous ne l’avez pas copié ou supprimé.": "Text stays here until copied or deleted.",
    "Texte de dictée à récupérer": "Dictation text to recover",
    "Revérifier les permissions macOS si le collage ou l'enregistrement cesse de fonctionner.": "Re-check macOS permissions if pasting or recording stops working.",
    "Mise à jour disponible": "Update available",
    "Installer et redémarrer": "Install and restart",
    "Téléchargement…": "Downloading...",
    "Télécharger": "Download",
    "Démarrer une dictée": "Start a dictation",
    "Définissez un raccourci": "Set a shortcut",
    "Démarrer une réunion": "Start a meeting",
    "Transcrire en direct avec l’audio système": "Transcribe live with system audio"
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
