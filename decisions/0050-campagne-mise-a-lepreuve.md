# ADR-0050 — Campagne de mise à l'épreuve adversariale du système

**Date :** 2026-05-30
**Statut :** Acceptée (cadrage — scénarios en sessions dédiées)

---

## Contexte

Le système a été **validé propriété par propriété** : SEF-1→SEF-6 (P1–P6), SEF-7.1/7.2/7.3 (forgerie causale, flood borné, robustesse reconstructeur), 14 scénarios `poc/scenarios/S1–S14`. Mais chaque validation est conduite **sous faute unique contrôlée ou happy-path**. Aucune campagne ne soumet le système à des conditions **adversariales, compositionnelles et soutenues**.

La discipline du projet (L68 : un jalon de faisabilité n'instancie pas la propriété ; L82 : une propriété tenue par sur-garantie n'instancie pas l'architecture spécifiée) impose une étape épistémique distincte : **arrêter de valider, commencer à attaquer**. Une mise à l'épreuve qui ne peut pas échouer ne prouve rien (corollaire L68). Cet ADR cadre cette campagne : axes, critères de **falsification** (qu'est-ce qui compte comme le système qui ÉCHOUE), non-objectifs, et pièges méthodologiques.

### Faits vérifiés déterminants pour le cadrage

- **F1 — L'inférence du PoC Linux est stubbée.** `InferenceBackend` simule la durée par `tokio::time::sleep` (model_id `"sleepy"`). Saturer le « mur d'inférence » C1 (spec/07) saturerait un mock — sans valeur. Conséquence : l'axe plafonds est **coupé** (D6).
- **F2 — Le rate-limit `CapabilityDenied (0x14)` droppe de l'information sous flood.** `actor.rs:829-895` : au-delà de 100 refus/agent/1s, un événement **agrégé** est émis qui enregistre `cap_id` + `count` mais **omet le champ `resource`** ; au-delà de 101, **silence total** (`should_emit = false`). C'est une confused-deputy candidate concrète (cible de l'axe 1).
- **F3 — Le store réel n'instancie pas la séparation CAS/index (L82, ADR-0049 §D2).** P6 tient par sur-garantie ACID (transaction redb à 4 tables), pas par l'append atomique sur log séparé qu'ADR-0038 §3 exige. **Red-teamer un système dont l'architecture diffère de la spec, c'est red-teamer la mauvaise cible** — d'où le gate soundness préalable (D2).
- **F4 — Substrat : le système complet vit sur le PoC Linux.** seL4 est clos (ADR-0049) et ne porte que P6/I4/W^X/isolation-processus. Scheduler, causalité, caps, rollback, supervision : `poc/runtime/`, `poc/scenarios/`. La campagne cible donc Linux (D5), avec garde-fou de non-transférabilité (D7).

---

## Décision

### D1 — Périmètre : un gate + deux axes. Substrat Linux PoC.

La campagne comporte **un gate préalable obligatoire** et **deux axes d'attaque** :

- **Gate — Audit de soundness** (préalable, bloquant) : appliquer la lentille L68/L82 à la suite SEF/scénarios. Pour chaque PASS, le verdict valide-t-il **l'invariant spécifié** ou un **proxy** (ou une sur-garantie qui masque la non-instanciation) ?
- **Axe 1 — Interactions entre défenses** (sécurité / P4) : recherche de confused-deputy inter-mécanismes.
- **Axe 3 — Crash concurrent à invalidation de cache** (atomicité / P6) : régime power-loss-like sur le store, N actions en vol.

(La numérotation conserve « axe 3 » pour traçabilité avec le cadrage initial à 4 axes ; l'axe 2 plafonds est coupé, D6.)

### D2 — Gate soundness : préalable bloquant, pas axe optionnel

On ne lance pas un red-team coûteux sur des cibles dont la conformité à la spec n'est pas établie. Le gate :

1. **Énumère** chaque propriété PASS (P1–P6, SEF-7) et l'invariant qu'elle prétend valider.
2. **Confronte** le verdict au code : l'oracle teste-t-il l'invariant, ou un proxy observable plus faible / une garantie plus forte qui masque ?
3. **Produit un verdict par propriété** : `INSTANCIÉE` / `PROXY` / `SUR-GARANTIE` (avec, dans les deux derniers cas, la cible réelle non instanciée).

**Le gate a déjà un échec connu** (P6 / L82 : sur-garantie ACID, séparation CAS/index non instanciée). Ce n'est pas une hypothèse, c'est un constat — d'où le statut de gate : il **recadre ce que les axes 1 et 3 ont le droit de prétendre**. Un crash test (axe 3) sur la transaction redb monolithique ne dit rien sur la robustesse de l'architecture journal-append-only visée. Le gate inscrit cette limite **avant** l'attaque.

**Critère de sortie du gate** : chaque propriété de la suite porte un verdict explicite `INSTANCIÉE/PROXY/SUR-GARANTIE`. Tant que ce n'est pas fait, axes 1 et 3 ne démarrent pas.

### D3 — Axe 1 : interactions entre défenses (pas « tout simultané »)

**Reformulation impérative.** Un agent combinant N vecteurs *simultanément* ne falsifie rien de plus que N agents mono-vecteur, et il détruit la capacité de diagnostic. La valeur n'existe que s'il y a **interaction** entre deux mécanismes de défense : l'un affame-t-il, masque-t-il, ou désarme-t-il l'autre ? [Hardy 1988, « The Confused Deputy »].

**Deux sous-cibles distinctes — à ne pas confondre.** L'axe 1 falsifie **deux propriétés différentes**, qui exigent **deux oracles différents** (la confusion entre les deux était un trou de la première rédaction, corrigé ici) :

- **1a — Isolation P4 (défaut d'isolation).** Le système ÉCHOUE si (a) une escalade de cap hors scope **réussit** (une cap est accordée à tort), OU (c) une **contamination cross-agent** est observable (un agent lit/écrit l'état d'un autre). **Oracle 1a : l'état du `CapabilityStore` + le kv_store par agent**, observés indépendamment du log. Une cap accordée à tort EST une mutation d'état observable ; une lecture cross-agent l'est aussi. Cet oracle est constructible : il y a un état à inspecter.

- **1b — Fidélité du log d'audit sous flood (défaut d'observabilité, PAS d'isolation).** Cible concrète F2 : `check()` (`actor.rs`) est une fonction **pure** — un refus de cap ne mute **rien**, sa seule trace est l'événement `0x14`. Le rate-limit droppe le champ `resource` (agrégé, >100) puis silence tout (>101). **Scénario falsifiant** : un agent inonde ≥ 100 refus bénins en 1 s, puis tente le refus malveillant qu'il veut cacher → agrégé sans `resource` ou silencé. Le système ÉCHOUE (1b) si un refus **tenté** n'est **pas attribuable** dans le log. **Oracle 1b : un témoin hors-bande au point de décision** — instrumenter le site de `check`/refus pour émettre, vers un canal de test *non rate-limité et hors du log causal*, un enregistrement `(agent, resource, perm)` de chaque tentative refusée, **avant** que `emit_cap_denied` n'applique son rate-limit. L'oracle compare {tentatives refusées vraies} vs {refus attribuables dans le log}. 1b échoue ssi l'écart est non vide.

**Distinction de portée à inscrire au verdict (piège L82-like).** Un échec 1b ne prouve **pas** que P4 (isolation) est violée — la cap est correctement refusée, seulement mal journalisée. 1b falsifie la **fidélité du log d'audit**, une propriété d'**observabilité**, pas d'isolation. Ne jamais présenter un échec 1b comme « P4 cassée » : ce serait inverser un invariant (CLAUDE.md). « Observer l'état des caps » (oracle 1a) est structurellement **incapable** de falsifier 1b — c'est pourquoi l'oracle hors-bande est obligatoire.

**Vecteurs à instruire en paires** (pas en bloc) : forgerie causale en rafale × rate-limit `0x14` (→ 1b) ; flood de refus × refus malveillant unique (→ 1b) ; tentative d'escalade × flood concurrent (→ 1a sous bruit). Chaque paire : les deux mécanismes sont-ils **indépendants**, ou l'un est-il l'angle mort de l'autre ?

### D4 — Axe 3 : crash concurrent à invalidation de cache (régime ADR-0027 §D3)

**Cible : le vrai trou jamais testé** — l'atomicité quand le **serveur de store meurt aussi** (pas seulement le runtime), en **régime concurrent** (N actions en vol). C'est la dette convergente ADR-0027 §D3 / ADR-0043 §69 (et non une « dangling reference de rollback », qui n'existe pas comme dette documentée).

**Niveau de crash : machine / cache invalidé** (décision 2026-05-30). Un `kill -9` du process laisse le page-cache du noyau hôte intact : un commit « durable » jamais écrit sur disque survit quand même → **durabilité fantôme** (piège L32, identique au « QEMU virtio-blk non recevable »). Seul un crash qui **invalide le cache** (VM power-off, ou `echo 3 > drop_caches` + kill) ajoute de l'information. C'est faisable sur Linux (contrairement à seL4/QEMU, où power-loss reste hors scope α).

**Le système ÉCHOUE (axe 3) si**, après recovery : (a) un commit **acké** est absent ou non reconstructible ; OU (b) un commit **partiel** est visible (état intermédiaire observable) ; OU (c) un `parent_ids` pointe vers du vide (référence pendante). Le log content-addressed post-recovery doit être un **préfixe valide** de la séquence ackée. Concevable → pas du théâtre.

**Garde-fou de recevabilité** : un crash niveau process (page-cache intact) n'est **pas** recevable pour cet axe — il ne falsifie pas plus que SEF-4. Le verdict n'est valide que sous invalidation de cache effective.

### D5 — Ordre d'exécution (ADR-0001 : P4 ≻ P2 ≻ P3 ≻ P6 ≻ P5 ≻ P1)

**Gate soundness → Axe 1 (P4) → Axe 3 (P6).**

- Gate d'abord : gratuit (analyse, pas de harness), invalide ou recadre les axes.
- Axe 1 ensuite : P4 est la tête d'ADR-0001 (condition de viabilité du modèle) ; un échec ici est le plus coûteux.
- Axe 3 après : P6 ≻ P5/P1, critère de falsification net.

### D6 — Non-objectifs (assumés, justifiés)

- **Plafonds C1/C2/C3 (ex-axe 2)** : coupés. C1 saturerait un stub (F1) ; le cap C2 est non recevable (calibré sur hardware non représentatif, sous-estimé ×3–4, L32 / spec/07 §150). Renvoyés à une campagne ultérieure conditionnée à : inférence réelle **et** cap C2 recalibré sur hardware représentatif.
- **Propriétés non ciblées par un axe (périmètre en négatif)** : **P1** (densité — pas une propriété de sûreté, plafond P1 lié à C1 stubbé), **P2** (rollback), **P3** (lookup — robustesse reconstruction déjà couverte par SEF-7.3), **P5** (déterminisme) ne sont **pas** des cibles de falsification d'axe. Justification : ADR-0001 ordonne P4 ≻ P2 ≻ P3 ≻ P6 ≻ P5 ≻ P1 ; la campagne attaque d'abord les deux propriétés dont l'échec est le plus coûteux (P4, tête) et le plus nettement falsifiable (P6), et dont des cibles concrètes existent (F2, ADR-0027 §D3). **Précision sur P2** : les paires de l'axe 1 peuvent *impliquer* la machinerie de rollback (vecteur « rollback abusif »), mais la **falsification** reste l'isolation (1a) ou la fidélité d'audit (1b) — **P2 n'est pas la propriété falsifiée**. Une campagne P2/P3/P5 dédiée est différée, non annulée.
- **Frontière LLM** : non-objectif. spec/08 §0.1 est explicite — le DAG garantit *happened-before*, **pas** la connaissance effective ni la « bonté » sémantique de la sortie LLM. Un LLM produisant une décision nuisible **dans son périmètre de caps** n'est pas une violation de propriété système ; c'est le périmètre que le contrat sémantique exclut. Confondre le modèle de menace OS (agent = WASM arbitraire hostile) avec un modèle d'alignement serait une erreur de cadre. Seul résidu légitime : un LLM dont la sortie *tente* d'exploiter une host function — déjà couvert par l'axe 1 (redevient un WASM hostile).

### D7 — Garde-fou de non-transférabilité Linux ↔ seL4

Tout résultat de robustesse obtenu sur Linux est **non transférable à seL4 sans re-validation**, et inversement les garanties seL4 (W^X réel, isolation VSpace matérielle) **ne sont pas** présentes sur Linux. L'isolation P4 testée à l'axe 1 est l'**isolation par capability logicielle du runtime**, pas l'isolation matérielle. Tout verdict doit le nommer : « P4 sous adversaire WASM, isolation logicielle Linux » — pas « P4 » nu. Sinon la campagne reproduit le biais L68 à son échelle.

---

## Pièges méthodologiques (à neutraliser dans chaque scénario)

Trois artefacts du PoC Linux peuvent faire passer un système fragile pour robuste :

1. **Page-cache du noyau (L32 redux)** — neutralisé par D4 (crash niveau machine / cache invalidé). Sans cela, durabilité fantôme.
2. **Inférence stubbée (F1)** — neutralisé par D6 (axe 2 coupé). Saturer un stub mesure une file d'attente, pas une dégradation réelle.
3. **Rate-limit comme masque (F2)** — c'est *l'objet* de la sous-cible 1b, pas un piège à éviter. Point de constructibilité critique : l'oracle qui compte les `0x14` ne peut **pas** falsifier 1b (il EST l'angle mort) ; et l'oracle « état des caps » (1a) ne le peut pas non plus (un refus tenté ne laisse aucune trace d'état — `check()` est pur). Seul un **témoin hors-bande au point de décision** (D3, oracle 1b), émis avant le rate-limit et hors du log causal, falsifie 1b. Écrire SEF-9 contre l'oracle « état des caps » pour 1b reproduirait le théâtre L68. L'oracle 1a (état des caps) reste correct pour 1a (escalade/contamination), qui *ont* un état à observer.

---

## Critères de falsification (synthèse)

| Composant | Le système ÉCHOUE si… | Oracle / recevabilité |
|-----------|------------------------|------------------------|
| Gate soundness | un PASS valide un proxy / une sur-garantie et non l'invariant | déjà échoué pour P6 (L82) — constat ; domaine fini (P1–P6 + SEF-7), terminable |
| **Axe 1a** (isolation P4) | escalade de cap hors scope réussit ; OU contamination cross-agent | **état du `CapabilityStore` + kv_store**, indépendant du log |
| **Axe 1b** (fidélité log d'audit) | un refus **tenté** est non attribuable dans le log (rate-limit l'a droppé/silencé) | **témoin hors-bande au point de décision** (avant rate-limit) ; un échec 1b ≠ P4 violée (observabilité, pas isolation) |
| Axe 3 (P6) | commit acké absent/non-reconstructible ; OU commit partiel visible ; OU `parent_ids` pendant | crash niveau machine (cache invalidé) obligatoire |

---

## Support et séquencement

**ADR de cadrage (celui-ci) d'abord ; scénarios ensuite.** Le critère de falsification (L68) doit être dans l'ADR, pas découvert pendant l'implémentation — sinon une famille SEF-8/S15+ serait du code sans critère d'échec, exactement le théâtre à éviter. Ordre des sessions de code : (1) gate soundness — produit le verdict `INSTANCIÉE/PROXY/SUR-GARANTIE` par propriété ; (2) axe 1 — scénario confused-deputy `0x14` ; (3) axe 3 — harness crash machine concurrent.

Famille de scénarios : `SEF-8` (soundness gate), `SEF-9` (axe 1), `SEF-10` (axe 3) — ou `S15+` selon la convention ADR-0021.

---

## Conséquences

- **TODO.md** : nouvelle section « Mise à l'épreuve adversariale » avec gate + 2 axes, ordre D5, critères D-falsification.
- **spec/08** : non amendé — la campagne *exerce* le modèle de menace, ne le modifie pas. Le non-objectif LLM (D6) cite §0.1 existant.
- **spec/07** : non amendé — les plafonds restent décrits ; leur mise à l'épreuve est explicitement différée (D6), pas annulée.
- **ADR-0027 §D3** : non amendé — l'axe 3 *exerce* le régime power-loss-like sur Linux (testable, contrairement à seL4 où il reste α). Un PASS axe 3 ne lève PAS α sur seL4 (D7).
- **ADR-0049** : cohérent — le gate soundness opérationnalise le constat D2 (séparation non instanciée) en l'étendant à toute la suite.

---

## Questions tranchées dans cet ADR (étaient bloquantes)

1. **Que signifie « mettre à l'épreuve » ?** → Gate soundness + 2 axes adversariaux, avec critère de falsification par composant (D1–D4).
2. **Inférence réelle ou stub ?** → Stub (F1) → axe plafonds coupé (D6).
3. **Niveau de crash axe 3 ?** → Machine / cache invalidé, régime concurrent (D4). Process-only non recevable.
4. **Frontière LLM dans le scope ?** → Non-objectif, justifié spec/08 §0.1 (D6).
5. **Substrat ?** → Linux PoC, garde-fou de non-transférabilité (D5, D7).
6. **Ordre ?** → Gate → axe 1 (P4) → axe 3 (P6) (D5).

---

## Références

- `decisions/0001-priorite-proprietes.md` — ordre P4 ≻ P2 ≻ P3 ≻ P6 ≻ P5 ≻ P1 (D5)
- `decisions/0027-durabilite-log-vs-contentstore.md` §D3 — régime power-loss/concurrent (cible axe 3)
- `decisions/0043-integration-verticale-c6.md` §69 — régime crash borné mono-agent seL4 (contraste)
- `decisions/0049-cloture-poc-sel4.md` §D2 — séparation CAS/index non instanciée (origine du gate)
- `decisions/0021-convention-scenarios.md` — convention scénarios (famille SEF-8+/S15+)
- `spec/08-modele-menace.md` §0.1 — happened-before vs knowledge (non-objectif LLM), TCB déclaré
- `spec/07-plafonds-architecturaux.md` §C1, §150 — plafonds (différés, D6)
- `lab/LESSONS.md` L32 (page-cache masque le signal), L68 (test qui ne peut échouer = théâtre), L82 (sur-garantie ≠ instanciation)
- `poc/runtime/src/actor.rs:829-895` — rate-limit `0x14` (confused-deputy, F2/axe 1b)
- `poc/runtime/src/actor.rs` `CapabilityStore::check` (fonction pure : un refus ne mute aucun état → seule trace = log → oracle 1b hors-bande obligatoire)
- `poc/runtime/src/lib.rs` — `InferenceBackend` stubbé `tokio::time::sleep` (F1)
- [Hardy 1988] « The Confused Deputy » — interaction entre mécanismes de défense (axe 1)
