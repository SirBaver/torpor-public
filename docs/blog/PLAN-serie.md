# Plan éditorial — Série de blog Torpor

**Statut :** document de pilotage interne (exclu de l'export public, comme les mémos de cadrage).
**Origine :** pivot stratégique validé par l'utilisateur le 2026-06-19 — abandon de la présentation interne devant collègues au profit d'une série d'articles de blog. Axes produits par l'agent `marketing` (câblé anti-survente), cadre de release par synthèse du mémo `memo-revue-strategique-2026-06.md` (O1–O6).

**Garde-fou maître :** la série vend une **méthode de recherche système honnête** autant qu'un artefact. Chaque article déclare son régime (R1 effets / R2 ressources), son statut (prouvé / conçu-non-livré / hors-scope) et porte sa preuve. La frontière LLM reste un non-objectif partout : on contrôle effets et ressources, jamais la qualité sémantique des décisions de l'agent. Voir `style-guide-anti-survente.md`.

---

## Phase 0 — Verrous préalables (avant le 1er article public)

| # | Verrou | Qui tranche | Bloque |
|---|--------|-------------|--------|
| 1 | ✅ Attribution copyright — **tranchée 2026-06-19 : nom réel « Joey Leonard »** (déjà en place dans les README FR/EN d'os-public ; rien à modifier) | utilisateur | ~~tout push/article public~~ levé |
| 2 | Position seL4 — A (jalon futur) vs B (réserves permanentes) — mémo O2 | `architect` | crédibilité des articles 4/6/7 (bornes Linux non transférables) |
| 3 | Règle O6 dans le style-guide (RocksDB = PoC Linux mesuré ; redb = cible seL4 ; jamais confondus) | acquis | tout claim de stack |

> Verrou #2 : le pivot blog **est** le trigger d'instruction prévu par O2. Décision instruite par l'agent `architect` à partir du 2026-06-19.

## Phase 1 — Infrastructure éditoriale

- **Plateforme — TRANCHÉE (utilisateur, 2026-06-19) : GitHub Pages.** Contrainte posée : gratuit + lecture sans inscription. GitHub Pages co-localise articles + code + releases + tags (les permalinks épinglés que citent les articles marchent nativement). **Conséquence technique à câbler** : GitHub Pages (Jekyll) **ne rend PAS Mermaid automatiquement** comme la vue Markdown du repo — il faut soit inclure `mermaid.js` dans le layout, soit embarquer les **SVG pré-rendus** (8/8 figures déjà rendues valides via `mmdc`). Inconvénient assumé : peu de découverte organique → un cross-post miroir (dev.to) reste possible plus tard, GitHub Pages = source de vérité.
- **Langue** *(utilisateur, en attente)* : FR d'abord puis EN, ou bilingue dès le départ. Termbase EN ratifié (GATE) déjà disponible (`../os-public-en/TERMBASE.md`).
- **Licence du texte** : CC-BY-4.0 (cohérence avec les docs du projet).
- **Posture repo — TRANCHÉE (verdict architect 2026-06-19) : modèle (I), publication en une fois + une release par article.** Voir Phase 4. La posture C « incrémentale » envisagée initialement est **périmée** par un constat factuel : les exports `torpor-public(-en)` sont **déjà complets** et leur historique git contient tout le corpus — retenir des fichiers au fil des articles ne réduirait la surface d'exposition que pour qui ne sait pas faire `git log` (sécurité par obscurité sur un dépôt public, et incohérence vérifiable avec le garde-fou maître d'honnêteté). On assume la publication, on ne la mime pas.

## Phase 2 — Calendrier éditorial

| # | Article (titre de travail) | Régime | Preuve-socle | Audience |
|---|---|---|---|---|
| 1 | *Votre OS croit qu'un agent IA est un humain à un terminal* | méta | pari #5 (supervision asymétrique) | décideur / praticien |
| 2 | *La flèche entre deux décisions EST un hash* (DAG causal) — **climax technique** | R1 | P3a p99 23 µs @ 10⁶ ; pari #1 | ingé système / chercheur |
| 3 | *Annuler 500 décisions en 17 ms* (rollback) | R1 | P2 SEF-2 17–20 ms @ depth 500 | ingé / praticien |
| 4 | *Le coût d'un agent endormi* (densité, **avec sa nuance**) | R2 | P1a ×4500–7375 **PARTIEL** | décideur / ingé |
| 5 | *On a essayé de tromper notre propre garde-frontière* (capabilities + red team) | R1 | SEF-3/9 confused-deputy | ingé / sécu |
| 6 | *Couper le courant à 4 moments précis, 40 fois* (atomicité crash seL4) | R1 | P6, 40 scénarios / 4 kill points | ingé / chercheur |
| 7 | *Pourquoi pas Docker, SQLite, ou Unix ?* (les 5 paris) | mixte | loi overhead R²=0.988 ; pari #2 | ingé / chercheur |
| 8 | *Trois fois, nos données ont contredit notre modèle* (la méthode) — **climax narratif** | méta | ADR-0032/0033/0034 | chercheur / décideur |

**Logique d'ordre :** ouvrir par le *pourquoi* (1) ; poser tôt le temps-fort le plus visuel (2) ; placer la densité (4) *après* deux preuves fortes pour que la nuance R2/PARTIEL soit lue en confiance, pas comme une dérobade ; monter la tension (5→6) ; récapituler (7) ; finir sur la méthode (8) — laisser le lecteur sur la crédibilité. **Article 2 = climax technique, article 8 = climax narratif** : les séparer encadre la série par « pourquoi » → « comment on sait que c'est vrai ».

**Cadence suggérée :** 1 article / 2 semaines (laisse le temps de la passe de preuve + relecture). 8 articles ≈ 4 mois.

### Détail des axes (accroche / preuve / limite honnête)

**Article 1 — thèse fondatrice.** Accroche : un humain ouvre un shell, tape, attend, ferme ; un agent autonome vit des heures, décide sans témoin — pourtant on lui donne un OS dessiné pour le premier cas. Limite : prototype de recherche, pas un produit ; on ne remplace pas Linux, on isole un besoin qu'il ne sert pas par construction.

**Article 2 — DAG causal infalsifiable (matière #1).** Accroche : la dépendance entre deux décisions d'agent n'est pas une ligne de log éditable, c'est un hash ; l'historique est rejouable et tamper-evident. Limite : infalsifiable = on ne peut réécrire sans que ça se voie ; ce n'est pas « l'agent décide bien » — l'audit porte sur la traçabilité, pas la justesse.

**Article 3 — rollback transactionnel.** Accroche : « il s'est trompé sur 500 pas — on rembobine l'état local, tout ou rien, en 20 ms ». Limite (critique) : couvre l'**état local**, pas les effets externes déjà émis ; la compensation saga est un **non-goal documenté**.

**Article 4 — densité (l'honnêteté EST l'angle).** Accroche : un agent dormant ne coûte pas comme un agent actif (état sur NVMe, réveil à la demande). Limite centrale : régime R2, densité *hébergée* ≠ *active*, **non transférable seL4**, P1 = propriété **sacrifiable** jusqu'à 3× (jamais un 5× ferme brandi).

**Article 5 — capabilities + red team.** Accroche : on a monté une attaque confused-deputy contre notre propre système ; l'isolation a tenu, et la red team a trouvé un trou d'audit qu'on a corrigé. Limite : une red team interne n'est pas une preuve d'imprenabilité — c'est une réfutation tentée et documentée, trou compris.

**Article 6 — atomicité crash sur seL4.** Accroche : on tue le système à 4 points de kill, 40 fois ; à chaque fois le commit est complet ou absent, jamais à moitié. Limite : « démontré sur seL4 » = QEMU AArch64, 11 jalons d'intégration — pas un déploiement matériel ; moteur = **redb**, pas RocksDB (O6).

**Article 7 — les 5 paris falsifiables.** Accroche : chaque choix de stack est un pari dont la condition de réfutation est écrite *avant* l'expérience ; le plus démonstratif = Wasmtime+Tokio vs Docker, loi `overhead(N)=9.65−54/N` KB (R²=0.988). Limite : un pari gagné dans nos conditions n'est pas une loi universelle ; pas de dénigrement de Docker.

**Article 8 — la méthode comme produit.** Accroche : trois fois, la donnée nous a donné tort, et c'est dans le dépôt, versionné, avec la condition de révision (ADR-0032/0033/0034). Limite : ce n'est pas une méthode « qui prouve qu'on a raison » — elle rend nos erreurs visibles et révisables ; la rigueur a coûté une mesure (P3a seL4 refusée sur substrat invalide).

### Axes DÉCONSEILLÉS (ne pas écrire)

- **« Comment notre OS rend les agents IA dignes de confiance / sûrs ».** Traverse la frontière LLM (non-objectif) : on ne contrôle pas la qualité des décisions. Expose le flanc le plus indéfendable et contamine les articles voisins.
- **« Torpor vs Linux : le benchmark » (comparatif frontal généraliste).** Invite une comparaison hors périmètre + chiffres « X× plus rapide que Linux » non substantiés. Le plancher de viabilité est une *condition de cohérence déjà franchie*, pas un argument flashy. Le contraste par pari (article 7) couvre le besoin sans le risque.

## Phase 3 — Pipeline de production par article

1. **Corps** → agent `writer` (transforme la matière validée en prose) ou `andragogue` si l'article vise la pédagogie (articles 1, 8).
2. **Passe de preuve** → vérifier que chaque chiffre/claim a son `S<N>` / ADR / `results/` réel et à jour. Re-`cargo audit` si on republie des artefacts code (cf. règle « à refaire le jour de la publication »).
3. **Passe anti-survente** → agent `marketing` relit contre les 9 survente-pièges (`style-guide-anti-survente.md`).
4. **Publication** = tagger `blog-NN-slug` (FR et EN) + publier les deux release notes jumelles (voir Phase 4). Le dépôt étant déjà public depuis T0, « publier » ne signifie plus exposer des fichiers, mais poser le point de citation stable de l'article.

## Phase 4 — Modèle de release & distribution

**Décision tranchée (architect, 2026-06-19) : modèle (I) — publication en une fois + une release par article. Décision de process, sans ADR** (elle ne touche aucun invariant de l'OS ; la *position* qu'elle diffuse, elle, est ADR-0065).

### Gating (ordre strict)

Les verrous Phase 0 doivent être **soldés AVANT** le tag parapluie `v0.1-public` et la mise en ligne de l'article 1 — **pas de publication anticipée de l'article 1** :

- Verrou #2 (position seL4 A/B) : ✅ soldé (ADR-0065 = B).
- Verrou #3 (règle O6) : ✅ acquis (style-guide).
- **Verrou #1 (copyright) : ✅ levé (2026-06-19).** Décision utilisateur : publication sous le **nom réel « Joey Leonard »**, déjà inscrit dans les README FR/EN d'os-public(-en) (« © 2026 Joey Leonard. »), licences Apache-2.0 + CC-BY-4.0 cohérentes. Le rappel de principe reste valable : sous (I), publier = pousser le dépôt **en entier**, et l'attribution devient **non réversible** (clones/archives tiers) — d'où l'importance d'avoir tranché l'identité *avant* le push. C'est fait.
- **Passe sensible ajoutée au go** : `cargo audit` + triage CVE par atteignabilité **frais au jour du tag parapluie** (pas étalé) — (I) rend tout le code citable dès T0.

Séquence : **(verrous Phase 0 clos) → (tag `v0.1-public` + release + article 1) → (release `blog-02` + article 2) → …**. L'ordre des *tags* suit l'ordre des *articles* (jamais de `blog-06` avant `blog-01`).

### Schéma de tags

- **`v0.1-public`** — tag parapluie à T0 (état complet curé, point de citation racine).
- **`blog-NN-slug`** par article (`NN` zéro-paddé), ex. `blog-02-dag-causal`, `blog-04-densite-dormante`. Tag = **curseur de citation**, pas promesse d'immobilité : il pointe `v0.1-public` tant que le dépôt ne bouge pas, ou le commit de correction si une passe de preuve a réaligné un artefact (la note le signale).
- **Double langue = releases jumelles** : chaque article = deux releases (`os-public` FR + `os-public-en` EN), **même tag**, permalinks dans chaque langue. Slugs EN issus du **termbase ratifié (GATE)**, jamais improvisés.

### Release note — carte claim → preuve → démo (format imposé)

Trois invariants **non négociables** par note : **(a)** substrat de mesure nommé dans la même ligne que la borne (R-blog-1) ; **(b)** moteur nommé — RocksDB = PoC Linux / redb = cible seL4 (R-blog-2, O6) ; **(c)** permalink épinglé à un tag, **jamais à `main`** (un lien `main` est falsifiable post-publication — ce que l'article 2 dénonce).

```
# blog-02 — La flèche entre deux décisions EST un hash
Régime : R1 · Statut : prouvé · Substrat : Linux/RocksDB/NVMe consumer (R-blog-1)

## Claim → Preuve
| Claim | Borne | Preuve (permalink @tag) | Substrat |
|-------|-------|-------------------------|----------|
| Lookup causal p99 | 23 µs @ 10⁶ | results/S…/verdict.json · ADR-00NN | Linux/RocksDB |

## Reproduire
$ git clone … && git checkout blog-02-dag-causal
$ CXXFLAGS="-include cstdint" cargo run -p … --bin …   # cellule autonome
# attendu : p99 ≈ 23 µs sur NVMe consumer ; dépend du substrat (R-blog-1)

## Limites (R-blog-3 / O6)
- Infalsifiable = non-réécriture détectable, PAS « l'agent décide juste ».
- Borne conditionnelle par construction ; aucune mesure seL4-native planifiée (ADR-0065).
```

### Risque d'exposition n°1 au go (hors O2 déjà tranché)

**La confusion RocksDB ↔ redb par agrégation inter-articles.** Un lecteur hostile juxtapose les bornes RocksDB/Linux (articles 2/3/4/7) et l'atomicité crash redb/seL4 (article 6) → « ils prétendent que leurs 23 µs tiennent sur seL4 ». Sous (I), tout est citable dès T0, donc l'agrégation est possible immédiatement. **Couverture :** (i) l'invariant « moteur nommé » de chaque note fournit le démenti à même la citation ; (ii) `decisions/0065` + le style-guide R-blog-1..4 sont exposés **dès `v0.1-public`** (la défense est en ligne avant l'attaque) ; (iii) l'ordre des tags = ordre des articles (densité/seL4 jamais avant que 1 et 2 soient en ligne avec leurs notes).

### Structure d'article & illustration (recadrage utilisateur 2026-06-19)

Le **code est publié d'un coup** (modèle I) — les curieux épluchent le dépôt, les patients attendent les articles. Conséquence sur la forme de chaque article :

- **Corps lisible sans rejouer.** Aucune commande dans le fil de lecture. Les commandes de reproduction sont regroupées en **fin d'article**, section « Reproduire » sautable, miroir du bundle.
- **Bloc « Spec & doc concernées »** obligatoire : chaque article pointe vers sa spec (`spec/`), ses ADR (`decisions/`), ses mesures (`results/`) et son code.
- **Illustration = figures** (le primaire). Sources **texte versionnables** : **Mermaid** pour les diagrammes (rend nativement sur GitHub) ; **script de plot** depuis `results/.../verdict.json` pour les courbes. Traçabilité via `figures/SOURCES.md`. Mêmes garde-fous que le texte : diagramme = légende « schéma conceptuel » ; graphe de mesure = substrat nommé (R-blog-1), densité « hébergée ≠ active » (art. 4). Si la plateforme finale ne gère pas Mermaid → pré-rendu SVG.

### Bundle compagnon `examples/blog-NN-slug/` (additif, use case strict — verdict architect)

Purement additif : aucun fichier du cœur runtime touché, démos/bins existants réutilisés tels quels. Structure : `REPRODUCE.md` (reproduction autoritaire = vrai système au tag) · `expected/` (transcripts réels horodatés : commit + tag + host/substrat + date) · `illustration/` **optionnel** (snippet du *principe* ; son absence est un signal, pas un trou) · `figures/` (+ `SOURCES.md`).

- **Source de vérité** : le code exécutable canonique vit dans le bundle ; l'article le **cite** (`> Cellule canonique : examples/blog-NN/ @tag`), ne le duplique pas. La passe de preuve (Phase 3 §2) vérifie l'égalité avant chaque tag.
- **`illustration/` ≠ preuve** : règle **R-blog-5** du style-guide (bandeau d'étiquetage, aucun chiffre, nom non-système, jamais dans la release note). Articles 3 et 5 : ajouter la ligne « ne couvre pas \<ce qui fait la preuve\> ».
- **Spectre de faisabilité** : art. 2 (illustration autonome OUI) · art. 4 (données + script de plot) · art. 3/5 (snippet de principe + transcript) · **art. 6 seL4 : PAS d'illustration autonome** (toolchain non réductible → transcript réel + figure, assumé franchement) · art. 1/7/8 (figures + liens).

### Distribution (après les premiers articles)

Mémo O4. À armer **seulement** après publication des premiers articles : annonce HN / lobste.rs / listes seL4-WASM. La série est elle-même un meilleur véhicule d'annonce qu'un repo silencieux — chaque article est un point d'entrée thématique.

---

*Source des axes : agent `marketing`, 2026-06-19. Cadre de release : `docs/memo-revue-strategique-2026-06.md` (O1–O6).*
