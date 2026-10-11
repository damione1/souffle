# SOU-278 — essai court avec la webcam

Le **probe séparé** compare brut, FDAF et Apple sur trois scènes de 30 secondes.
La Nightly habituelle garde son traitement existant. La Nightly construite avec
`--native-aec` utilise l'intégration [SOU-279](../sou-279/README.md) dans l'app.
Les trois essais déjà faits dans la
Nightly constituent la baseline du FDAF ; leurs fichiers mixtes ne permettent pas
de reconstituer le micro brut. Ce parcours enregistre les pistes nécessaires à
l'écoute comparative, sans gold, profils ASR ni téléchargement.

## Lancer les trois paires

Depuis le worktree `sou-277` :

```sh
./scripts/build-aec-probe.sh
probe="$PWD/app/target/aec-probe/Souffle AEC Probe.app/Contents/MacOS/souffle-aec-probe"
"$probe" devices
./scripts/check-aec-probe.sh 'UID exact de la webcam'
```

`devices` lit noms, UID et fréquences sans ouvrir de capture. Copier l'UID de la
webcam dans `inputs` (son nom peut être « Unknown USB Audio Device »). Le micro
macOS par défaut peut rester celui du Mac, capot fermé : le probe exige l'UID
demandé et refuse un autre périphérique.

Le lanceur attend Entrée avant chaque prise, annonce la scène et le mode, puis
crée un dossier privé neuf sous `~/Souffle-QA/SOU-278-...`. `q` arrête le parcours.
Il n'est jamais exécuté par les builds ou les tests.

Choisir Standard dans les contrôles micro du probe. Conserver volume, webcam,
position et distance. Remettre la même vidéo au même point avant chaque prise
distante. Commencer au message `RECORDING`, après trois secondes de préparation.

| Scène | Action | Écoute |
|---|---|---|
| Vidéo seule | Vidéo sur les haut-parleurs, rester silencieux | Voix distante restante dans brut, FDAF et Apple |
| Voix seule | Vidéo en pause, parler, faire une pause puis reprendre | Voix locale préservée, syllabes et reprises coupées |
| Les deux | Même vidéo et voix locale, avec pause/reprise | Voix distante restante **et** voix locale conservée pendant le chevauchement |

Une demande d'autorisation peut faire échouer le premier essai. Après avoir
accordé micro et capture système, relancer dans un dossier neuf ; aucun reset TCC
ni changement de profil Nightly n'est nécessaire. Conserver toute erreur exacte.

## Une prise individuelle

Avec `$qa` désignant un dossier parent existant et `$uid` l'UID choisi :

```sh
"$probe" check "$qa/remote-baseline" baseline remote --input-uid "$uid"
"$probe" check "$qa/remote-apple" apple remote --input-uid "$uid"
"$probe" check "$qa/local-baseline" baseline local --input-uid "$uid"
"$probe" check "$qa/local-apple" apple local --input-uid "$uid"
"$probe" check "$qa/double-baseline" baseline double --input-uid "$uid"
"$probe" check "$qa/double-apple" apple double --input-uid "$uid"
```

Les dossiers existants sont refusés. `--seconds` accepte 1 à 60 ; `--notes`
conserve les observations. `--output-uid` peut préciser la sortie annoncée par
`devices`, qui doit correspondre à la sortie système utilisée par le navigateur.
Le probe ne change aucune préférence globale pour faire réussir une prise.

## Résultats

Écouter à volume de lecture constant : `raw.wav` et `fdaf.wav` dans chaque dossier
`*-baseline`, puis `apple.wav` dans le dossier Apple correspondant. Brut et FDAF
partagent la même prise ; le FDAF réutilise le DSP existant hors ligne. Apple est
une prise physique distincte. `them.wav` permet de vérifier le contenu distant.

Remplir `notes.txt`, puis transmettre les observations et les `manifest.json` /
`runtime.log`. Garder les WAV privés hors Git. Le manifeste contient les UID
demandés/résolus, fréquences, périphériques macOS par défaut, readbacks Apple
avant/après démarrage et OSStatus. Toute identité différente entraîne un refus.
Voice Processing doit être activé, AGC désactivé et sortie native active. Le tap
n'est jamais envoyé aux haut-parleurs ; Apple et FDAF restent séparés.

Ces prises portent `purpose: listening-check` et `comparison_eligible: false`.
Les commandes de scores et de transcription du probe les refusent, et la matrice
formelle les exclut. Une transcription Whisper facultative peut utiliser
directement le [CLI existant](../sou-277/README.md#asr-and-metrics) sur un WAV ;
elle ne constitue pas un WER ni une attribution des mots à un locuteur.

## Limites et vérification

La sélection Apple concerne ses AudioUnits arrêtées, après activation du
traitement vocal et avant `prepare/start` : `CurrentDevice`, scope global,
bus 1 entrée / bus 0 sortie, comme dans [WebKit](https://github.com/WebKit/WebKit/blob/main/Source/WebCore/platform/mediastream/cocoa/CoreAudioCaptureUnit.cpp#L398).
CPAL sélectionne le même UID. Le format natif doit être mono f32 ; un matériel
incompatible est refusé avec diagnostic. Les fréquences nominales ne sont pas
forcées. Les limites de synchronisation du tap et de CPU en build dev de
[SOU-277](../sou-277/README.md) restent applicables.

La réussite des appels API ne démontre pas l'annulation du son du navigateur.
L'efficacité sur la webcam reste en attente des écoutes de Damien. Aucun essai
matériel n'a été exécuté automatiquement pendant l'implémentation.
