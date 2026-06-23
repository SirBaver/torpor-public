# ADR-0065 — Position de transférabilité : réserves permanentes (B)

- **Statut :** Acceptée
- **Date :** 2026-06-19
- **Décideurs :** agent architect (verdict O2), utilisateur (déclencheur)
- **Amende :** reformule `spec/01-vision.md`, `spec/04-hypotheses.md`, `spec/07-plafonds-architecturaux.md` ; renvois ajoutés dans ADR-0050, ADR-0052
- **S'appuie sur :** ADR-0049 §D1/§D3 (clôture PoC seL4), ADR-0046 (déclencheur matériel D-reopen/power-loss), ADR-0048 §D3 (PKI / second producteur)
- **Liés :** ADR-0049, ADR-0046, ADR-0048, ADR-0050, ADR-0052

---

## Contexte

Le mémo de revue stratégique (`docs/memo-revue-strategique-2026-06.md`, observation O2) a identifié une **position par défaut non écrite** : les documents normatifs portent 9 réserves « non transférable » et 7 mentions « substrat seL4-natif », toutes indexées sur un *prototype de stockage seL4-natif* qu'aucun trigger armé ne planifie (le PoC seL4 est clos et gelé, ADR-0049 : « déclencheurs objectifs épuisés »). Toutes les bornes chiffrées publiées (P1a ×4 500–7 375 RAM dormante, P2 17–20 ms, P3a p99 23 µs, cap 14 agents/s) sont **mesurées sur un substrat explicitement non-cible** (Linux/RocksDB/NVMe consumer) ou sont des hypothèses non validées.

Cette asymétrie est structurelle : les réserves promettent implicitement un « jour où l'on mesurera sur la stack réelle », mais aucun chemin armé n'y mène. O2 proposait de trancher entre deux positions, toutes deux honnêtes :

- **(A) Jalon futur** — un C.x « stockage seL4-natif minimal mesurable » redevient l'objectif du prochain cycle constructif. Coût élevé (cf. effort C.1→C.11). Bénéfice : transforme toutes les réserves en mesures (ou réfutations).
- **(B) Réserves permanentes** — le projet acte que son livrable est *un substrat Linux mesuré + une direction seL4 démontrée + des bornes conditionnelles assumées comme telles*. Zéro coût constructif, exige une passe de reformulation.

**Déclencheur.** O2 armait le trigger : « publication effective OU première question externe sur la transférabilité » *instruit* la décision. Le pivot vers une **série d'articles de blog** (décision utilisateur 2026-06-19) est ce déclencheur — une série technique qui publie des bornes chiffrées rendrait la question de transférabilité la première attaque prévisible d'un lecteur attentif.

## Décision

**(B) Réserves permanentes.**

Le livrable définitif du projet est : **un substrat Linux mesuré + une direction seL4 démontrée (chaîne C.1→C.11 fonctionnelle, ADR-0049 §D1) + des bornes conditionnelles par construction.** Les 9 réserves « non transférable » ne sont **pas** des dettes en attente de mesure : elles constituent le **périmètre final assumé** du projet.

## Justification

1. **(A) contredit ADR-0049 et le YAGNI du projet.** ADR-0049 §D1 déclare le PoC seL4 clos pour épuisement des déclencheurs objectifs ; sa Justification pose que coder par anticipation sans risque à lever est « la définition de la violation YAGNI que le projet refuse ». Un jalon C.x « stockage seL4-natif mesurable » instancierait la séparation CAS/index (direction ADR-0049 §D3a), dont le déclencheur est le GC des orphelins — non atteint. Ouvrir A referait ce qu'ADR-0049 a refusé pour (a), B-fort, et C.12+.

2. **Le trigger de blog n'arme pas A.** O2 dit que la publication *instruit* la décision, pas qu'elle la tranche vers A. Une question externe sur la transférabilité est une demande de **clarté de position**, pas une demande de mesure seL4. B y répond complètement (« bornes conditionnelles par construction, hors périmètre ») ; A y répondrait par un cycle de plusieurs mois pour des chiffres qui resteraient un point unique sur un substrat encore non-final (QEMU non recevable, ADR-0046 ; board réel indisponible).

3. **B est déjà la position de fait du corpus.** ADR-0050 §Conséquences, ADR-0052 §D3, spec/01 et spec/07 portent déjà « abandonnée, pas reportée », « décision explicite », « ne sera pas poursuivie sur ce substrat ». B ne change pas le périmètre : il **retire les épingles temporelles résiduelles** qui le contredisent.

4. **Cohérence O5.** O5 acte « le cycle constructif est terminé ». A rouvre un cycle constructif majeur — frontalement contraire. B est un geste hors-code, conforme.

5. **Asymétrie de réversibilité.** B ne ferme aucune porte ; il cesse seulement de *promettre* qu'on la franchira. A engage le coût immédiatement et irréversiblement.

## Conséquences

**Reformulations appliquées (neutralisation des épingles temporelles).** La cible n'est pas de supprimer les réserves (correctes et load-bearing) ni de réécrire les paragraphes, mais de convertir les tournures « jusqu'à / si un jour » en énoncés décisionnels fermés :

- `spec/01-vision.md` — borne 14 agents/s « reste la référence jusqu'à un prototype seL4-natif » → « est la référence du projet : borne Linux/NVMe conditionnelle par construction, aucune mesure seL4-native n'est planifiée » ; mention « Mesure sur substrat seL4 hardware réel : non exécutée » → « hors périmètre, conditionnelle par construction ; D-P3a reste dormant sur déclencheur matériel ».
- `spec/07-plafonds-architecturaux.md` — « recalibré si un substrat seL4-natif est un jour mesuré » → « recalibré sur un autre substrat ; aucune mesure seL4-native n'étant planifiée, 14 agents/s est la borne de référence définitive sur substrat Linux/NVMe » ; « le cap reste 14 agents/s jusqu'à mesure sur un prototype seL4-natif » → « borne Linux/NVMe conditionnelle par construction, périmètre final ».
- `spec/04-hypotheses.md` — frontière ajoutée : la borne GPU « reste à mesurer » est un **déclencheur infra réel** (GPU), distinct de la non-transférabilité seL4 qui, elle, est hors périmètre.
- `decisions/0050`, `decisions/0052` — renvois actant que les réserves sont permanentes, non des dettes de mesure.

**Règles de communication (série de blog).** Cette position est opérationnalisée dans `docs/blog/style-guide-anti-survente.md` (règles R-blog-1..4) :
- **R-blog-1** — toute borne chiffrée nomme son substrat de mesure dans la même phrase.
- **R-blog-2** — la bifurcation de moteur est nommée, pas masquée (RocksDB = PoC Linux ; redb = cible seL4 — règle O6).
- **R-blog-3** — la conditionnalité se présente comme une force de méthode, jamais comme une dette (interdit : « en attendant un prototype seL4 »).
- **R-blog-4** — la direction seL4 se revendique sur ce qui est démontré (chaîne C.1→C.11 fonctionnelle), jamais sur des perfs seL4.

## Condition de réouverture

B est **réversible**, pour ne pas durcir en « jamais seL4 ». Réouverture sur déclencheur **objectif** :
- déclencheur matériel réel (board ARM / NVMe passthrough — ADR-0046 §D-reopen/power-loss), ou
- second producteur de modules / PKI (ADR-0048 §D3).

Grammaire identique à tous les dormants du projet : *« pas de mesure seL4-native planifiée, réouverture sur déclencheur objectif »* — pas une fermeture définitive.
