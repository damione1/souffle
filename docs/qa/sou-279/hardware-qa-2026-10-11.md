# QA webcam, haut-parleurs et Whisper — 11 octobre 2026

Damien a effectué les trois enregistrements dans la Nightly signée installée
dans `/Applications`, puis l'a quittée avec ⌘Q. L'agent a vérifié les
transcriptions déjà sauvegardées et les logs ; il n'a pas déclenché de capture
ni retranscrit les fichiers audio. Les contenus privés et l'UID matériel ne
sont pas reproduits ici.

## Provenance

- Sources du bundle : `107140d474f53db97d2438fe515eb4426dafcecc`.
- `SouffleNativeAEC=true`, identité `com.souffle.desktop.nightly`.
- UUID arm64 : `C01F10F3-290C-37B4-98CA-CA315A554ED1`.
- SHA-256 du binaire :
  `49b516e74f9816b3a217463f9a79d91be1f836c16f77430c3874505b7c266e31`.
- `codesign --verify --deep --strict --verbose=2` : PASS.
- MacBook Pro M4 Pro, macOS 27, capot fermé confirmé par les logs.
- Webcam USB comme entrée réelle, haut-parleurs intégrés comme sortie.
- Whisper Large V3 Turbo, backend whisper-rs ; aucune prise Parakeet dans ce QA.
- Débuts des prises : 2026-10-12 à 02:10:49, 02:12:05 et 02:13:01 UTC
  (11 octobre, 22:10–22:13, America/Toronto).

## Transcriptions persistées

Les nombres sont ceux des segments finaux dans `segments`, regroupés par
réunion et `speaker`. La référence est constituée des trois prises « Part 1 »
avec CPAL/FDAF ; les prises « Part 2 » utilisent Apple.

| Scène | Référence : durée, Me / Them | Apple : durée, Me / Them | Observation |
|---|---|---|---|
| Vidéo seule, utilisateur silencieux | 40 s, 8 / 8 | 50 s, 2 / 8 | Les longues duplications dans Me disparaissent ; deux fragments courts subsistent. |
| Voix seule | 35 s, 6 / 0 | 32 s, 7 / 0 | La parole locale reste dans Me ; aucun segment Them. |
| Vidéo et voix locale | 39 s, 7 / 7 | 38 s, 3 / 5 | Me contient la parole locale en français, Them la vidéo en anglais, sans duplication textuelle évidente entre les pistes. |

Pour la vidéo seule, Me passe de 299 à 32 caractères tandis que Them passe de
381 à 605 caractères. Un fragment Me reprend quelques mots de la vidéo ;
l'autre contient une attribution de locuteur probablement hallucinée. On ne
peut pas distinguer une fuite acoustique d'une hallucination ASR sur cette seule
preuve textuelle. Le résultat est une amélioration nette, pas une annulation
parfaite. Dans le transcript sauvegardé, les deux fragments Me sont situés à
0–4,31 s puis 4,31–8,31 s ; aucun autre segment Me n'est enregistré ensuite
sur cette prise de 50 s. Ces timestamps ne permettent pas d'attribuer le résidu
à une phase d'adaptation de l'AEC.
Les durées et passages vidéo diffèrent : ces comptes ne sont ni un
WER, ni une mesure d'atténuation acoustique, ni un benchmark normalisé.

## Activation et capture

Les trois sessions confirment le même chemin :

- `Meeting backend=Apple VoiceProcessingIO, FDAF=disabled`.
- Entrée webcam et sortie haut-parleurs : périphériques demandés identiques
  aux readbacks avant démarrage, après démarrage et pendant le fonctionnement.
- Voice processing actif sur les deux nœuds, bypass désactivé, AGC désactivé.
- Format client demandé et négocié : float32 mono non entrelacé, 16 kHz.
  Le format initial Apple à 48 kHz / 9 canaux n'est pas moyenné : Apple fournit
  le flux client mono, vérifié sur les deux côtés du graphe.
- Source native de sortie silencieuse ; la vidéo audible provient de son
  application. Them reste capturé par le tap système à 48 kHz.

| Scène | Callbacks micro | Samples perdus | Erreurs |
|---|---:|---:|---:|
| Vidéo seule | 498 | 0 | 0 |
| Voix seule | 322 | 0 | 0 |
| Vidéo et voix | 379 | 0 | 0 |

Le tap est sain sur les deux prises avec vidéo. Son diagnostic « Silent » sur
la prise voix seule est cohérent avec l'absence volontaire de son système.
Les trois réunions sont sauvegardées et leurs sessions terminées.

## Fermeture signée

Après le ⌘Q demandé à Damien, le log du 2026-10-12 à 02:19:17 UTC contient,
dans cet ordre : `Whisper model unloaded`, `Engine actor shut down cleanly`,
puis `Transcription engine released; actor shutdown completed`.
Le processus Nightly n'est plus présent. Cela valide la fermeture avec Whisper
chargé sur ce bundle, après arrêt des trois enregistrements. Cela ne remplace
pas un test de Quit pendant une transcription ou une capture active.
Aucun nouveau rapport de crash Soufflé n'a été trouvé dans le répertoire
DiagnosticReports au moment de la vérification.

## Couverture des critères d'intégration SOU-279

| Critère | Preuve nommée | Limite |
|---|---|---|
| AC1 : chemin réunion Apple opt-in | `build_opt_in_selects_only_the_meeting_backend` et backend Apple dans les trois logs | La dictée conserve son chemin existant ; aucune dictée Apple testée. |
| AC2 : webcam et sortie réellement sélectionnées | Readbacks avant/après start et en fonctionnement sur les trois prises ; `disappearing_explicit_webcam_cannot_be_replaced_by_auto_resolution` | Débranchement physique pendant une prise non effectué. |
| AC3 : pas de deuxième AEC, pistes distinctes | FDAF désactivé dans les trois logs ; `apple_route_flips_and_mic_replacement_keep_fdaf_off_and_lanes_separate` ; transcriptions voix seule et double parole | Deux fragments Me subsistent sur vidéo seule. |
| AC4 : erreur explicite et conservée, sans fallback silencieux | `capture_failure_survives_setup_answers_stop_and_allows_next_session` et tests des diagnostics AudioCapture ; refus des signatures et formats non conformes | Aucun échec injecté pendant les trois prises réussies de Damien. |
| AC5 : arrêt, files bornées et changements de route | Trois arrêts et trois sauvegardes ; `native_destinations_preserve_me_samples_and_bound_overflow`, test des changements de route et contrôles périodiques hors callback | Reconfiguration matérielle pendant capture non mesurée. |
| AC6 : bundle signé, guide, logs et gate locale | Provenance et signature ci-dessus, guide `README.md`, résultats des commandes dans `verification.md` | La matrice acoustique SOU-277 reste un livrable distinct et incomplet. |

## Portée de la conclusion

Le QA Whisper/webcam/capot fermé montre une amélioration utilisable de la
séparation Me/Them, avec des résidus sur la vidéo seule. Il valide aussi le
démarrage répété du graphe et la fermeture ⌘Q après enregistrement.
Restent non mesurés : Parakeet sur ce chemin matériel, débranchement de la
webcam pendant une capture, coexistence avec une application de visioconférence,
macOS 13 et la matrice acoustique répétée du protocole SOU-277. Le traitement
Apple reste opt-in pour les Nightly de réunion.
