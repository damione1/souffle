const fs = require('fs');

function addTranslations(file, newStrings) {
  const data = JSON.parse(fs.readFileSync(file, 'utf8'));
  Object.assign(data.onboarding, newStrings);
  fs.writeFileSync(file, JSON.stringify(data, null, 2));
}

addTranslations('src/lib/i18n/fr.json', {
  "interview_title": "Personnalisation",
  "interview_subtitle": "Aidez Souffle à reconnaître vos mots habituels en 5 questions. Ignorable.",
  "interview_skip": "Passer",
  "interview_next": "Suivant",
  "interview_q_name": "Quel est votre prénom ?",
  "interview_q_job": "Quel est votre métier ?",
  "interview_q_people": "Quels sont les prénoms de vos proches ou collègues ?",
  "interview_q_jargon": "Quel est le jargon de votre métier ?",
  "interview_q_tone": "Quel ton ou style Souffle doit-il utiliser lors du nettoyage de la dictée (ex: tutoiement, formel, concis) ?",
  "interview_ton_polish_warning": "Le nettoyage de dictée est désactivé dans les réglages. Ce style sera enregistré mais ignoré tant qu'il n'est pas réactivé."
});

addTranslations('src/lib/i18n/en.json', {
  "interview_title": "Personalization",
  "interview_subtitle": "Help Souffle recognize your usual words in 5 quick questions. Optional.",
  "interview_skip": "Skip",
  "interview_next": "Next",
  "interview_q_name": "What is your first name?",
  "interview_q_job": "What is your job?",
  "interview_q_people": "What are the first names of your colleagues or relatives?",
  "interview_q_jargon": "What is your industry's jargon?",
  "interview_q_tone": "What tone or style should Souffle use when polishing your dictation (e.g. casual, formal, concise)?",
  "interview_ton_polish_warning": "Dictation polish is disabled in settings. This style will be saved but ignored until it is re-enabled."
});
