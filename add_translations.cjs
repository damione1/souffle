const fs = require('fs');

function addTranslations(file, newStrings) {
  const data = JSON.parse(fs.readFileSync(file, 'utf8'));
  Object.assign(data.onboarding, newStrings);
  fs.writeFileSync(file, JSON.stringify(data, null, 2));
}

addTranslations('src/lib/i18n/fr.json', {
  "interview_title": "Dictionnaire",
  "interview_subtitle": "5 questions optionnelles pour aider Souffle à reconnaître vos mots habituels.",
  "interview_skip": "Passer",
  "interview_next": "Suivant",
  "interview_q_name": "Quel est votre prénom ?",
  "interview_q_job": "Quel est votre métier ?",
  "interview_q_people": "Quels sont les prénoms de vos proches ou collègues ?",
  "interview_q_jargon": "Quel est le jargon que vous utilisez souvent ?",
  "interview_q_tone": "Quel ton ou style Souffle doit-il utiliser lors du nettoyage de la dictée (ex: tutoiement, formel, concis) ?",
  "interview_ton_polish_warning": "Le nettoyage de dictée est désactivé. Ce style sera enregistré mais ignoré tant qu'il n'est pas activé."
});

addTranslations('src/lib/i18n/en.json', {
  "interview_title": "Dictionary",
  "interview_subtitle": "5 optional questions to help Souffle recognize your usual words.",
  "interview_skip": "Skip",
  "interview_next": "Next",
  "interview_q_name": "What is your first name?",
  "interview_q_job": "What is your job?",
  "interview_q_people": "What are the first names of your colleagues or relatives?",
  "interview_q_jargon": "What jargon do you frequently use?",
  "interview_q_tone": "What tone or style should Souffle use when polishing your dictation (e.g. casual, formal, concise)?",
  "interview_ton_polish_warning": "Dictation polish is disabled. This style will be saved but ignored until it is re-enabled."
});
