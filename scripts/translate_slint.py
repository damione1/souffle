import os
import re

translations = {
    "Méthode de collage": "Paste Method",
    "Le champ ciblé utilise l'accessibilité pour insérer dans la sélection. Utilisez la frappe simulée si une app refuse le ⌘V synthétique (terminaux, champs sécurisés).": "The target field uses accessibility to insert into selection. Use simulated typing if an app rejects synthetic ⌘V (terminals, secure fields).",
    "Délai de collage (ms)": "Paste Delay (ms)",
    "Délai de collage en millisecondes": "Paste delay in milliseconds",
    "Appuyez une fois pour démarrer ou arrêter la dictée.": "Press once to start or stop dictation.",
    "Maintenez pour enregistrer, relâchez pour arrêter.": "Hold to record, release to stop.",
    "Ne pas afficher le HUD flottant pendant une dictée ou une réunion. Déplacez-le par glisser-déposer ; la position est mémorisée et reste à l'écran si la résolution change.": "Do not show the floating HUD during a dictation or meeting. Drag to move it; the position is saved and stays on screen if resolution changes.",
    "Votre raccourci sur une seule touche ne fonctionnera pas tant que l'Accessibilité n'est pas accordée.": "Your single-key shortcut will not work until Accessibility is granted.",
    "Vérifier les permissions": "Check permissions",
    "Jouer un son au démarrage et à l'arrêt de l'enregistrement.": "Play a sound when starting and stopping recording.",
    "Volume des sons de dictée": "Dictation sound volume",
    "Résumés": "Summaries",
    "Confidentialité": "Privacy",
    "Mises à jour": "Updates",
    "Vérification…": "Checking...",
    "Vérifier les mises à jour": "Check for updates",
    "Vérification automatique": "Automatic Check",
    "Vérifier les mises à jour au démarrage.": "Check for updates on startup.",
    "Vérification automatique des mises à jour": "Automatic update check",
    "Générer le résumé": "Generate summary",
    "Régénérer": "Regenerate",
    "à l'instant": "just now",
    "Périphérique d'entrée": "Input Device",
    "Choisissez le microphone que Soufflé écoute et configurez le traitement audio.": "Choose the microphone Soufflé listens to and configure audio processing.",
    "Réinitialiser la liste": "Reset list",
    "Le périphérique choisi n'est plus disponible ; un autre est utilisé en attendant.": "The chosen device is no longer available; another is used in the meantime.",
    "Fréquence d'échantillonnage": "Sample Rate",
    "Fréquence native du périphérique sélectionné.": "Native sample rate of the selected device.",
    "Cette fréquence élevée peut perturber les appels en visioconférence pendant l'enregistrement.": "This high frequency can disrupt video calls during recording.",
    "Réinitialisation…": "Resetting...",
    "Réinitialiser à 48 kHz": "Reset to 48 kHz",
    "Ordre de préférence": "Preference Order",
    "Utilisé quand plusieurs micros sont branchés en même temps.": "Used when multiple mics are plugged in at the same time.",
    "Sinon, un casque Bluetooth n'est jamais sélectionné automatiquement (évite le mode mono basse qualité).": "Otherwise, a Bluetooth headset is never selected automatically (avoids low-quality mono mode).",
    "Intégration du calendrier": "Calendar Integration",
    "Afficher les réunions du jour sur l'accueil et rappeler avant qu'elles commencent.": "Show today's meetings on home and remind before they start.",
    "L'accès au calendrier a été refusé.": "Calendar access denied.",
    "Ouvrir les Réglages Système": "Open System Settings",
    "Proposer au début de la réunion": "Prompt when meeting starts",
    "Quand un événement commence et que l'audio système est actif, proposer de démarrer l'enregistrement.": "When an event starts and system audio is active, offer to start recording.",
    "Délai du rappel": "Reminder Delay",
    "Minutes avant un événement pour proposer de démarrer une transcription.": "Minutes before an event to offer starting a transcription.",
    "Délai du rappel en minutes": "Reminder delay in minutes",
    "Tous les calendriers sont inclus ; décochez ceux à masquer.": "All calendars are included; uncheck those to hide.",
    "Apprendre depuis les corrections après collage": "Learn from post-paste corrections",
    "Quand le collage auto est actif, retenir les corrections mot à mot faites dans l'app cible pour les dictées suivantes.": "When auto-paste is active, remember word-by-word corrections made in the target app for future dictations.",
    "Catégorie facultative": "Optional Category",
    "Aucune entrée.": "No entries.",
    "Débogage": "Debug",
    "Quantité de détails que Soufflé écrit dans le fichier journal.": "Amount of detail Soufflé writes to the log file.",
    "Journaux détaillés de transcription": "Verbose transcription logs",
    "Enregistre des détails supplémentaires sur la reconnaissance vocale pour le dépannage.": "Records extra details about speech recognition for troubleshooting.",
    "Fin du fichier journal actuel, actualisé toutes les quelques secondes.": "End of current log file, refreshed every few seconds.",
    "Aucune entrée de journal pour l'instant.": "No log entries yet.",
    "Activez Apple Intelligence dans les Réglages Système, puis rouvrez Soufflé.": "Enable Apple Intelligence in System Settings, then reopen Soufflé.",
    "macOS télécharge encore le modèle on-device ; réessayez plus tard.": "macOS is still downloading the on-device model; try again later.",
    "Apple Intelligence nécessite macOS 26 ou plus récent.": "Apple Intelligence requires macOS 15.4 or newer.",
    "Apple Intelligence nécessite une puce Apple Silicon.": "Apple Intelligence requires Apple Silicon.",
    "Le moteur qui rédige les résumés et nettoie la dictée.": "The engine that writes summaries and cleans up dictation.",
    "Moteur de résumé": "Summary Engine",
    "Réglages Système": "System Settings",
    "Aucun modèle trouvé.": "No model found.",
    "1 modèle trouvé.": "1 model found.",
    "Connecté": "Connected",
    "Non disponible": "Unavailable",
    "Réessayer": "Retry",
    "Modèle de résumé": "Summary Model",
    "Aucun modèle compatible installé.": "No compatible model installed.",
    "Bloc de code sélectionnable": "Selectable code block",
    "Conserver indéfiniment": "Keep forever",
    "Conservation audio des réunions": "Meeting Audio Retention",
    "Durée de conservation des enregistrements audio bruts.": "How long raw audio recordings are kept.",
    "Exporter les données": "Export Data",
    "Exporter réunions, dictées et journal dans un dossier.": "Export meetings, dictations and log to a folder.",
    "Afficher le dossier de données": "Show Data Folder",
    "Afficher dans le Finder": "Show in Finder",
    "Permet à Claude Desktop, Claude Code ou un autre client MCP de lire vos transcriptions et résumés. Tout reste sur ce Mac.": "Allows Claude Desktop, Claude Code or another MCP client to read your transcriptions and summaries. Everything stays on this Mac.",
    "Binaire introuvable. Compilez souffle-mcp pour le développement local.": "Binary not found. Build souffle-mcp for local development.",
    "À exécuter une fois dans un terminal pour enregistrer le serveur.": "Run once in a terminal to register the server.",
    "Inclure le son des haut-parleurs (l'autre côté d'un appel) dans les réunions.": "Include speaker audio (the other side of a call) in meetings.",
    "Arrêter l'enregistrement quand plus personne ne parle depuis longtemps.": "Stop recording when nobody has spoken for a long time.",
    "Ignorer les silences pour une transcription plus rapide et plus propre.": "Skip silences for faster and cleaner transcription.",
    "Vos dictées et meetings apparaîtront ici, du plus récent au plus ancien.": "Your dictations and meetings will appear here, from newest to oldest.",
    "Essayez d'autres mots-clés — la recherche couvre toutes les transcriptions.": "Try other keywords — search covers all transcriptions.",
    "Connectez le calendrier macOS pour des rappels et un démarrage en un clic.": "Connect macOS calendar for reminders and one-click start."
}

def translate_file(path):
    with open(path, "r", encoding="utf-8") as f:
        content = f.read()

    for fr, en in translations.items():
        content = content.replace(f'@tr("{fr}")', f'@tr("{en}")')

    with open(path, "w", encoding="utf-8") as f:
        f.write(content)

for root, _, files in os.walk("src-tauri/souffle-slint/ui"):
    for file in files:
        if file.endswith(".slint"):
            translate_file(os.path.join(root, file))
