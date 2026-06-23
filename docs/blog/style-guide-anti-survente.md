# Style-guide anti-survente — Série de blog Torpor

**Statut :** garde-fou de rédaction réutilisable. Toute passe de relecture d'un article (Phase 3 §3 du `PLAN-serie.md`) se fait contre ce document, via l'agent `marketing`.
**Sources :** garde-fous épistémiques du projet (régimes R1/R2, statuts, frontière LLM, F1/L68), 9 survente-pièges produits par `marketing` (2026-06-19), règles de communication R-blog-1..4 produites par `architect` lors du verdict O2 = **B (réserves permanentes)** (2026-06-19, → ADR-0065).

---

## Principe maître

La crédibilité est l'argument de vente. Sur un projet de recherche système, **une seule survente détectée détruit la valeur de preuve de tout le reste**. On vend fort, mais on ne vend que ce qui est vrai. La rigueur (limites nommées, régimes déclarés, conditionnalité assumée) n'est pas une faiblesse à cacher : c'est le produit.

## Les trois cadres obligatoires de tout claim

1. **Régime déclaré** — R1 (effets : P2/P3/P4/P6, toujours actif) ou R2 (ressources : P1/P5/C1/C2, inférence locale uniquement). Jamais les six propriétés en bloc. « L'OS garantit tout » est interdit.
2. **Statut déclaré** — *prouvé* (scénario vert / mesure publiée) vs *conçu, non livré* (intention de design) vs *hors périmètre* (non-goal documenté). Présenter une intention comme du livré disqualifie l'article.
3. **Frontière LLM = non-objectif** — on contrôle les *effets* et les *ressources* d'un agent, jamais la qualité sémantique de ses décisions. Un LLM qui décide mal *dans son périmètre de caps* n'est pas un échec du système.

---

## Règles de communication des bornes chiffrées (verdict architect O2=B → ADR-0065)

**R-blog-1 — Toute borne chiffrée nomme son substrat de mesure dans la même phrase.** Jamais de chiffre nu.
- « 17–20 ms » → « 17–20 ms sur le PoC Linux (Rust/Wasmtime/RocksDB, Ryzen 4650U + WD SN530) ».
- « ×4500–7375 » → « densité dormante mesurée Wasmtime vs Docker+Python sur Linux ».

**R-blog-2 — La bifurcation de moteur est nommée, pas masquée (règle O6).** Tout passage qui présente la stack sépare deux colonnes : *PoC Linux mesuré (RocksDB)* vs *cible seL4 (redb, Wasmtime no_std)*. Ne jamais laisser croire qu'une borne RocksDB/Linux prédit la latence redb/seL4. **Le changement de moteur EST l'une des raisons de la non-transférabilité** — c'est un argument de rigueur, pas un aveu.

**R-blog-3 — La conditionnalité se présente comme une force de méthode, jamais comme une dette.**
- *Autorisé* : « cette borne est conditionnelle par construction au substrat de mesure ; nous ne l'extrapolons pas à la cible seL4 et ne prévoyons pas de la mesurer là-bas — choix de périmètre assumé (B), pas un travail manquant. »
- *INTERDIT* : « en attendant un prototype seL4 », « il resterait à valider sur seL4 », « une fois la stack cible mesurée ». Ces tournures réarment une promesse que la position B a retirée et exposent le flanc exact qu'un lecteur attentif attaquera.

**R-blog-4 — La direction seL4 se revendique sur ce qui est réellement démontré.**
- *Livré côté seL4* : chaîne C.1→C.11 fonctionnelle (isolation 2-processus, P6/P6-N crash-processus, W^X, WASM non confié — ADR-0049 §D1). Dire « la viabilité de la direction est démontrée par une chaîne fonctionnelle ».
- *PAS livré* : latences/débits sur seL4, séparation CAS/index (cible non instanciée, ADR-0049 §D2). Ne PAS dire « les propriétés de performance tiennent sur seL4 ».

**R-blog-5 — Une illustration minimale n'est pas le runtime (bundles compagnons).** Un artefact pédagogique auto-portant (`examples/blog-NN/illustration/`) montre le *principe*, jamais le système réel. Tout fichier d'`illustration/` porte en première ligne (et en tête de son `README.md`) le bandeau exact :
```
ILLUSTRATION DU PRINCIPE — CE N'EST PAS LE RUNTIME TORPOR.
Ce fichier reproduit l'IDÉE de <propriété> en autonome (sans RocksDB/Wasmtime/le DAG réel).
La PREUVE vit dans REPRODUCE.md (le vrai système, cloné au tag, mesuré sur <substrat>).
Ce code n'établit aucune borne chiffrée.
```
Trois interdits absolus pour `illustration/` : (a) **aucun chiffre** (pas de « p99 », pas de « ×4500 » — toute borne appartient à la reproduction autoritaire, avec son substrat R-blog-1) ; (b) **aucun nom du système** comme type/fonction (`MerkleishDemo`, pas `TorporLog` — un nom non citable hors contexte comme « le code de Torpor ») ; (c) **jamais dans la release note** (qui ne lie que la preuve). Articles 3 et 5 : ajouter au bandeau la ligne *« cette illustration ne couvre pas \<ce qui fait la preuve\> : \<atomicité transactionnelle persistante / la résistance à une red team\> »* (le principe illustré est sémantiquement proche du claim mais en diffère sur ce qui fait sa valeur → survente par proximité).

---

## Les 10 survente-pièges (checklist de relecture)

1. **Le ×5 / ×7375 de densité présenté comme un chiffre de perf ferme.** Flanc le plus exposé. Toujours : régime R2 + « hébergée ≠ active » + « non transférable seL4 » + « propriété sacrifiable jusqu'à 3× ». Isoler ce chiffre de son contexte démolit l'article densité ET la crédibilité du reste.
2. **« seL4 » lu comme « en production sur du matériel ».** Toujours : QEMU AArch64, démontré via 11 jalons d'intégration, prototype de recherche.
3. **Coller RocksDB à seL4.** Erreur fatale (O6 / R-blog-2). RocksDB = PoC Linux ; redb = cible seL4. Jamais dans une même phrase de claim.
4. **Le rollback présenté comme rappelant des effets externes.** Toujours « état local » ; la compensation saga est un **non-goal documenté**. Sans cette limite, l'article rollback est attaquable en une phrase.
5. **« L'OS rend l'IA fiable / sûre / alignée ».** Interdit transverse (frontière LLM). On contrôle effets + ressources, pas la qualité sémantique.
6. **« Infalsifiable » glissant vers « sécurité totale ».** Le DAG prouve la *traçabilité* (on ne réécrit pas l'historique sans que ça se voie), pas l'imprenabilité ni la justesse des décisions.
7. **Les 6 propriétés revendiquées en bloc.** Jamais. Chaque article déclare son régime.
8. **La red team présentée comme preuve d'imprenabilité.** C'est une réfutation tentée et documentée (trou d'audit trouvé inclus) — pas un label de sécurité.
9. **Démos en rejeu maquillées en live.** Si un article s'appuie sur une démo à inférence stubbée, afficher `mode: rejeu` (F1/L68). Le rejeu prouve le contrôle des *effets*, pas une performance d'inférence.
10. **Illustration minimale lue comme le runtime réel** (variante code du piège #9). Un snippet pédagogique d'`illustration/` n'a ni RocksDB, ni le DAG réel, ni la vérification crypto complète — il montre l'idée, pas le système. Vérifier le bandeau R-blog-5, l'absence de chiffre, le nom non-système, et l'absence dans la release note.

---

## Deux axes INTERDITS (ne jamais écrire)

- **« Comment notre OS rend les agents IA dignes de confiance / sûrs ».** Traverse la frontière LLM. Le mot « confiance » ne s'applique qu'à la *traçabilité* et à l'*isolation des effets*, jamais au jugement de l'agent.
- **« Torpor vs Linux : le benchmark » (comparatif frontal généraliste).** Comparaison hors périmètre + chiffres non substantiés. Le plancher de viabilité est une condition de cohérence déjà franchie, pas un argument flashy. Le contraste par pari (article 7) couvre le besoin sans le risque.

---

## Voix et prose — recalibrage 2026-06-19 (retour utilisateur, contraignant)

Les règles anti-survente ci-dessus gouvernent **ce qui est vrai**. Elles ne sont **pas** un mandat d'écrire une négation par paragraphe. La première rédaction a dérivé en trois défauts que tout article doit désormais éviter :

**V1 — Montrer avant d'expliquer.** Le cœur d'un article est une **sortie réelle** du système (hashes réels, lignes de log réelles, code de sortie, table de mesure), placée dans le corps, pas en annexe « Reproduire ». On affiche l'artefact, *puis* on dit ce qu'il prouve. Une démonstration racontée en prose (« le système le détecte ») ne vaut rien à côté de la sortie qui le montre. Si on a fait tourner le code, la sortie est dans l'article.

**V2 — Affirmer, pas se renier.** Écrire ce que le système **fait**, à la forme affirmative. Le tic « pas X, mais Y », « ce n'est pas… », « X ≠ Y » est **proscrit comme figure récurrente**. L'honnêteté ne s'obtient pas en se contredisant à chaque phrase : elle s'obtient en **ne montrant rien de plus que ce qui tourne**. Une affirmation exacte est plus honnête qu'une affirmation suivie de son démenti.

**V3 — Une nuance par article, dite une fois.** La limite honnête se pose **une seule fois**, concrètement, sans s'excuser, idéalement en clôture. Pas de section « Les limites » à puces qui empile les ≠. Une phrase nette qui borne la portée du claim suffit, et elle vaut mieux que dix.

**V4 — Zéro tiret cadratin comme béquille.** Le tiret long (`—`) en incise est interdit comme connecteur par défaut. On coupe la phrase au point, ou on emploie une virgule, un deux-points, une parenthèse. Phrases courtes. Une idée par phrase.

> Test de relecture : compter dans l'article (a) les sorties réelles affichées, (b) les tournures négatives « pas/ne…pas/≠/sans », (c) les tirets longs. Cible : (a) ≥ 1 par article, (b) une poignée maximum dont **une seule** porte la nuance de fond, (c) zéro.

## Style (rappel marketing)

- Accroche concrète d'abord, abstraction ensuite. Une scène vaut mieux qu'une liste de features.
- Phrases courtes, verbes d'action, zéro remplissage.
- Le bénéfice avant le mécanisme ; le mécanisme juste en dessous pour qui creuse (format deux niveaux : décideur / technique).
- Chaque claim porte sa preuve : la sortie réelle dans le corps (V1), le pointeur (`S<N>`, `results/…`, ADR-xxxx) dans le bloc « Spec & doc ».
- On ne dénigre pas les alternatives (Linux, conteneurs, API LLM) : contraste factuel, en reconnaissant ce qu'elles font bien.

---

*Établi le 2026-06-19. Fusionne : axes `marketing` + verdict `architect` O2=B (ADR-0065). Recalibrage voix (V1–V4) 2026-06-19 sur retour utilisateur : montrer la sortie réelle, affirmer, une nuance, zéro tiret long.*
