# SEF-8 — Gate de soundness (ADR-0050 §D2)

**Date :** 2026-05-30
**Nature :** Audit (analyse, pas de harness — ADR-0050 §Support). Préalable bloquant aux axes 1 et 3.
**Méthode :** Pour chaque propriété PASS, confronter l'**oracle réel** (code) à l'**invariant spécifié** (spec/02). Verdict ∈ {`INSTANCIÉE`, `PROXY`, `SUR-GARANTIE`}. Lentille L68 (un test qui ne peut pas échouer ne prouve rien) + L82 (une propriété tenue par une garantie plus forte que spécifiée n'instancie pas l'architecture visée).

> **Convention de verdict**
> - **INSTANCIÉE** : l'oracle teste l'invariant spécifié, et un échec est concevable.
> - **PROXY** : l'oracle teste un observable plus faible que l'invariant ; l'écart est exploitable / non couvert.
> - **SUR-GARANTIE** : la propriété tient par une garantie plus forte que celle spécifiée, ce qui masque que l'architecture visée n'est pas instanciée.
> - Verdicts **scindés** quand isolation/audit ou substrats divergent.

---

## Tableau de synthèse

| Propriété | Oracle (code) | Invariant (spec/02) | Verdict |
|-----------|---------------|---------------------|---------|
| **P1a** densité hébergée | RSS/agent idle mesuré (~5 KB, CoW WASM) | ≥5× baseline Docker, RAM constante | **INSTANCIÉE** |
| **P1b** densité active | débit sous `SleepyBackend(2500ms)` | ≥2× débit, p99 borné | **PROXY** (inférence stubbée) |
| **P2** rollback (fonctionnel) | `hash_after == hash_at_k`, chaîne content-addressed | état restauré à T | **INSTANCIÉE** |
| **P2** rollback (complexité) | `rollback_path` = O(depth), borne murale ≤100ms à N=500 | **O(log N)** | **PROXY** (impl O(depth) ≠ O(log N) ; spec sur-revendique) |
| **P3a** lookup | `get(action_id)` p99 sur 10⁸ statiques | latence bornée indép. taille | **INSTANCIÉE** (pour la portée P3a) |
| **P4** isolation (allow/deny) | S9 : 10 enfants, accès couverts OK / non-couverts -1 | soundness accès + refus | **INSTANCIÉE** |
| **P4** audit (« 100% loggé ») | S9 : 20 refus < rate-limit 100/s → tous loggés | 100% des refus enregistrés | **PROXY** (faux sous flood — F2, cible axe 1b) |
| **P5** déterminisme | SEF-6 : 2 instances, AGENT_WAT, hash final + séquence | même état+sortie sous primitives substituables (S6) | **PROXY** (non-déterminisme non exercé) |
| **P6** atomicité — Linux | SEF-4 : oracle = préfixe valide du log ; 3 crashpoints | état ∈ {pré-tx, post-dernier-commit} | **INSTANCIÉE** (régime crash-processus) |
| **P6** atomicité — seL4 | redb : 4 tables, 1 transaction | append atomique sur log CAS séparé (ADR-0038 §3) | **SUR-GARANTIE** (L82) |
| **SEF-7.1** forgerie | forged_id absent de `parent_ids`, fail-closed -3 | existence O(1), fail-closed | **INSTANCIÉE** |
| **SEF-7.2** flood borné | 16 causes **injectées directement**, 17ᵉ via API → -2 | MAX_EXTRA_CAUSES borne le chemin réel | **PROXY** (accumulation bypasse l'API) |
| **SEF-7.3** robustesse reconstruct | boucle `parent_ids`, `Ok(None)` → WARN, jamais panic | pas de panic sur DAG incomplet | **INSTANCIÉE** (provisoire) |

**Bilan** : 5 INSTANCIÉE, 5 PROXY, 1 SUR-GARANTIE (+ verdicts scindés P2/P4/P6). Le gate a, comme prévu (ADR-0050 §D2), un échec connu d'avance (P6-seL4) et il en surface **quatre autres** non anticipés (P1b, P2-complexité, P4-audit, P5, SEF-7.2).

---

## Détail des verdicts non-INSTANCIÉE

### P1b — densité active = PROXY
`spec/02 §P1b` définit la métrique sous `SleepyBackend(delay_ms=2500)` — l'inférence est **stubbée par construction** (`tokio::time::sleep`). Le débit ≥2× est mesuré contre un mock ; le régime réellement borné par l'inférence (mur C1, spec/07) n'est jamais exercé. **Conséquence campagne** : c'est la raison du retrait de l'axe 2 (ADR-0050 §D6). Le verdict n'invalide pas P1b dans son périmètre déclaré (la spec assume le stub), mais il interdit d'extrapoler « débit actif » à un régime d'inférence réelle.

### P2 — complexité O(log N) = PROXY (spec sur-revendique)
`spec/02 §P2` affirme « complexité O(log N) où N est le nombre d'actions depuis le dernier commit barrier ». Le code (`poc/store/src/lib.rs:132`) documente explicitement `rollback_path` comme **O(tip.seq − target_seq)** = O(depth), chaque saut étant un point lookup RocksDB — une **traversée linéaire** de la chaîne de parents. SEF-2 ne valide que la borne **murale** (≤100ms) à N=500, qui passe parce que 500 lookups sont rapides — elle **ne valide pas** la complexité O(log N), et ne pourrait pas (elle mesure un temps, pas un ordre). La structure content-addressed à chaîne de parents simple est *fondamentalement* O(depth) ; O(log N) exigerait un skip-list / structure équilibrée non implémentée. **Recommandation** : corriger `spec/02 §P2` (« O(log N) » → « O(depth) »), ou acter la cible O(log N) comme non instanciée. Décision spec → architect.

### P4 — audit « 100% loggé » = PROXY (cible directe de l'axe 1b)
`spec/02 §P4` exige « 100% des tentatives d'accès non autorisées sont enregistrées dans le log causal ». S9 valide ce critère avec **20 refus** (10 enfants × 2 refus), très en-deçà du rate-limit `0x14` de **100 refus/agent/1s** (`actor.rs:829-895`). Sous flood (>100), le rate-limit émet un agrégé **sans le champ `resource`** puis silence tout (>101). Donc « 100% loggé » est **faux dans le régime floodé** et n'est testé que dans le régime non-floodé. C'est exactement F2 / la cible de l'axe 1b. **L'isolation (allow/deny) reste INSTANCIÉE** — la cap n'est jamais accordée à tort ; c'est l'**observabilité** de l'audit qui est proxy. **Recommandation** : `spec/02 §P4` doit qualifier le critère d'audit (« 100% loggé **sous le rate-limit ; au-delà, agrégation/silence par design** ») — la complétude d'audit absolue est non tenue. Décision spec → architect.

### P5 — déterminisme = PROXY
`spec/02 §P5` est une garantie **conditionnelle** : elle tient *parce que* toute source de non-déterminisme (horloge, aléa, inférence, réseau) passe par des primitives **substituables** (S6, `Clock`/`LogicalClock`, ADR-0028). SEF-6 exécute `AGENT_WAT`, dont l'en-tête du runner note explicitement : « les sources de non-déterminisme (horloge, réseau) ne sont **pas** exercées ». L'oracle compare le hash final + la séquence d'`action_id` de deux instances d'un agent qui **n'a aucune entrée non-déterministe** — un déterminisme trivial. Le **mécanisme** qui porte P5 (substitution de primitive en replay) n'est pas falsifié. **Recommandation** : un SEF-6-bis exerçant un agent qui *consomme* `Clock`/aléa, rejoué via `LogicalClock`, falsifierait réellement P5. Différé (ADR-0050 §D6 : P5 hors cibles d'axe).

### P6 — atomicité crash = INSTANCIÉE (Linux) / SUR-GARANTIE (seL4)
Verdict **scindé par substrat** — c'est le finding le plus important du gate.

- **Linux (SEF-4) = INSTANCIÉE.** Le chemin de commit (`actor.rs:1182-1211`) est **3 écritures séparées** : `put_block` → `put_snapshot` (ContentStore) → `log.append` (CausalLog) — PAS un WriteBatch unique. Trois crashpoints sont instrumentés, dont #2 « block orphelin » et #3 « ContentStore en avance sur log ». Le commit point atomique est l'**append du log** (un put RocksDB, atomique). L'oracle (`sef4_verify.rs:207`) prend `hash_after` du dernier LogEntry comme observable, et tolère par design les snapshots ContentStore orphelins en avance (non référencés, GC-ables) — conforme à ADR-0027 §D3 (« le log est la source de vérité pour la causalité observable »). L'invariant « état ∈ {pré-tx, post-dernier-commit} » est testé sous 3 crashs réels, orphelins inclus. **Un échec est concevable** (commit partiel visible, parent pendant) → pas du théâtre.
  - **Caveat 1 (tension spec)** : `spec/02 §P6` définit l'« état local » comme le **ContentStore** (Merkle DAG), tandis que l'oracle observe via le **log**. C'est principiel (ADR-0027), mais le texte spec/02 et l'oracle se réfèrent à deux objets différents — à réconcilier (le log est l'observable, le ContentStore peut porter des orphelins tolérés).
  - **Caveat 2 (non couvert par SEF-4)** : l'oracle ne **scanne pas** le ContentStore à la recherche d'orphelins en avance du log. C'est cohérent avec le modèle (orphelins = garbage), mais signifie que SEF-4 ne falsifie rien au niveau ContentStore — exactement le périmètre que l'axe 3 (crash machine concurrent) doit étendre.
  - **Régime** : crash-processus (page-cache noyau intact). C'est la base que l'axe 3 attaque en passant au crash machine (cache invalidé).

- **seL4 = SUR-GARANTIE.** Le store redb écrit 4 tables (blobs, headers, journal, seq) dans **une transaction ACID unique**. P6 tient par l'atomicité transactionnelle englobante — une garantie *plus forte* que l'append atomique sur log CAS séparé qu'ADR-0038 §3 spécifie. L'ordre (`TABLE_JOURNAL_A`) est autoritaire dans redb, donc la séparation CAS-autoritaire/index-reconstructible **n'est pas instanciée** (L82, ADR-0049 §D2). Verdict déjà acté.

### SEF-7.2 — flood borné = PROXY
Le test (`lib.rs:1346`) injecte les 16 premières causes **directement** dans `pending_extra_causes` via `state_mut().pending_extra_causes.push(*id)`, contournant l'API réelle `agent_add_cause` (et donc sa vérification d'existence). Seul le **17ᵉ** appel passe par le vrai chemin WAT → -2. La **borne au 17ᵉ** est donc testée via le chemin réel, mais l'**accumulation jusqu'à 16** (et la vérification d'existence sur ces 16) est scaffoldée. L'invariant « MAX_EXTRA_CAUSES borne le chemin réel » est partiellement proxifié. **Recommandation** : un test exécutant 17 appels `agent_add_cause` réels lèverait le caveat. Mineur.

---

## Recadrage des axes (effet du gate sur ADR-0050)

Le gate **recadre** ce que les axes ont le droit de prétendre :

1. **Axe 1b (fidélité audit) confirmé pertinent** : P4-audit = PROXY est précisément l'angle mort F2. L'axe 1b attaque une faille réelle, pas du théâtre.
2. **Axe 3 (crash P6) confirmé pertinent** : P6-Linux = INSTANCIÉE *seulement* en régime crash-processus (page-cache), et SEF-4 ne scanne pas les orphelins ContentStore. L'axe 3 (crash machine concurrent) étend exactement là où SEF-4 s'arrête.
3. **Axe 1a (isolation P4) a une base saine** : allow/deny = INSTANCIÉE → un échec 1a (escalade réussie) serait un vrai défaut, pas un artefact d'oracle faible.

## Findings hors axes (surfacés, non attaqués — ADR-0050 §D6)

Le gate surface trois proxies qui ne sont **pas** des cibles d'axe (P2/P5 hors scope), mais qui sont des **dettes de véracité spec/code** :

- **P2 §spec** : « O(log N) » contredit l'impl O(depth) documentée. → correction spec.
- **P4 §spec** : « 100% loggé » non tenu sous flood. → qualification spec.
- **P5** : déterminisme validé sur agent trivialement déterministe ; mécanisme S6 non exercé. → SEF-6-bis différé.
- **P6 §spec** : « état local = ContentStore » vs oracle observant le log. → réconciliation spec/02 ↔ ADR-0027.

**Ces quatre corrections de spec sont des décisions architecturales** (amender spec/02) → à porter à l'`architect` avant édition. Le gate les *constate*, il ne les tranche pas.

---

## Clôture du gate

Domaine fini (P1–P6 + SEF-7.1/7.2/7.3), chaque ligne porte un verdict argumenté → **gate clos** (critère de sortie ADR-0050 §D2 satisfait). Les axes 1 et 3 peuvent démarrer, avec les recadrages ci-dessus. Verdict net : **la suite de validation contient 5 proxies et 1 sur-garantie ; aucune propriété de sûreté (P4-isolation, P6) n'est *fausse*, mais P2-complexité et P4-audit sur-revendiquent au niveau spec, et P1b/P5/SEF-7.2 valident des observables plus faibles que l'invariant.**

---

## Références

- `decisions/0050-campagne-mise-a-lepreuve.md` §D2 (gate), §D6 (P2/P5 hors cibles)
- `decisions/0049-cloture-poc-sel4.md` §D2 (P6-seL4 sur-garantie), `decisions/0027-durabilite-log-vs-contentstore.md` §D3 (log = observable)
- `spec/02-properties.md` §P2 (O(log N) — à corriger), §P4 (100% loggé — à qualifier), §P5 (S6 conditionnel), §P6 (état = ContentStore)
- `poc/store/src/lib.rs:132` (rollback_path O(depth)), `poc/runtime/src/actor.rs:1182-1211` (3 écritures, crashpoints), `:829-895` (rate-limit 0x14)
- `poc/runtime/src/bin/sef4_verify.rs:207` (oracle log-centric), `sef6_runner.rs` (non-déterminisme non exercé), `lib.rs:1346` (SEF-7.2 injection directe), `:3210` (S9 isolation)
- `lab/LESSONS.md` L68 (test qui ne peut échouer = théâtre), L82 (sur-garantie ≠ instanciation)
