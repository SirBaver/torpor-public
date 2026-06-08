# ADR-0053 — Cadrage de la campagne adversariale P2 / P3 / P5

**Date :** 2026-05-30
**Statut :** Acceptée (cadrage — scénarios en sessions dédiées)

---

## Contexte

ADR-0050 §D6 a explicitement **différé, non annulé**, la mise à l'épreuve adversariale de P2 (rollback), P3 (traçabilité causale) et P5 (déterminisme). La campagne ADR-0050/0051 n'a attaqué que P4 (axe 1) et P6 (axe 3) — les deux propriétés dont l'échec est le plus coûteux (P4, tête d'ADR-0001) et le plus nettement falsifiable (P6). Le gate de soundness SEF-8 a néanmoins audité **toute** la suite et a produit un constat directement structurant pour la présente campagne (L87) :

- **P2 = PROXY.** L'oracle SEF-2 (S7) mesure un temps mur ≤ 100 ms à N fixe ; il ne falsifie aucun ordre de complexité, et le « hash identique » qu'il vérifie est **trivialement vrai par construction** sous content-addressing (le rollback repointe `last_snapshot` vers un `SnapshotHeader` *existant*, il ne reconstruit rien — cf. `S7/README.md §"Pourquoi un seul processus"`).
- **P3a = INSTANCIÉE** (lookup point, ADR-0026), mais sur **DB statique, lecture seule, sans write concurrent** — la portée la plus étroite (`S14/README.md §Portée`). P3b mesurée (T5-bis), P3c réservée (jamais implémentée).
- **P5 = PROXY (entrée triviale).** SEF-6 (S8) valide le déterminisme sur un agent qui **ne consomme aucune entrée non-déterministe** ; le mécanisme qui *porte* la garantie (substitution `Clock → LogicalClock`, S6) est exercé, mais sur un cas où il est le seul facteur de variabilité et où aucune primitive stochastique (LLM, entropie) n'entre dans la préimage du hash. ADR-0052 §Clôture sous-axe B a précisé le vrai déclencheur : faire entrer une valeur dérivée d'une primitive non-déterministe dans `state_bytes` (préimage `commit_barrier`, `actor.rs:1267-1317`) — **pas** via `kv_store`, qui n'est jamais sérialisé dans `data_hash`.

### État des oracles actuels et leurs limites

| Propriété | Oracle actuel | Verdict SEF-8 | Limite adversariale |
|-----------|---------------|---------------|---------------------|
| P2 | SEF-2 / S7 : 5 runs, N=1000, rollback à k=500, budget 100 ms | **PROXY** (borne murale) | Chaîne unique, payload fixe, mono-agent, pas de concurrence, hash trivial par construction |
| P3a | SEF-5 / S14 : 10⁴ lookups, N=10⁸, p99 ≤ 10 ms | **INSTANCIÉE** | DB statique, read-only, aucun write concurrent, aucun `action_id` forgé |
| P5 | SEF-6 / S8 : 2 instances, N=1000, `LogicalClock` identique | **PROXY** (entrée triviale) | Agent sans primitive non-déterministe ; LLM hors préimage hash par construction (ADR-0052) |

### Principe directeur (rappel ADR-0050 + L87/L88)

1. **Critère avant code** — l'oracle (et son point de constructibilité) est défini *dans cet ADR*, pas découvert pendant l'implémentation (corollaire L68).
2. **Gate de soundness en premier** — on ne red-teame pas une cible dont l'oracle est un proxy : SEF-8 a déjà signalé P2 et P5 comme PROXY ; le présent gate doit produire l'**oracle non-proxy** *avant* l'attaque, faute de quoi on attaque la mauvaise cible.
3. **Ne pas sur-vendre** — un échec sur une dimension (latence) ne falsifie pas une autre (intégrité). Tout verdict nomme la dimension, le substrat (Linux PoC, garde-fou de non-transférabilité ADR-0050 §D7), et distingue **occurrence** vs **sévérité construite** (L89).

---

## Décision

### D0 — Périmètre global : un gate + trois axes, dans l'ordre d'ADR-0001 (P2 ≻ P3 ≻ P5)

La campagne comporte **un gate de soundness préalable bloquant** (G) et **trois axes** (A-P2, A-P3, A-P5), exécutés dans l'ordre de priorité d'arbitrage ADR-0001 : **P2 ≻ P3a ≻ P5**. Substrat : Linux PoC (ADR-0050 §D5, garde-fou D7). Backend d'inférence : `OllamaBackend/qwen2.5:3b` quand un axe exige une primitive non-déterministe réelle (ADR-0052 §D2, garde-fou de non-transférabilité).

L'ordre n'est pas négociable : P2 est le différenciateur fonctionnel principal (ADR-0001), P3a le second, P5 l'avant-dernier rang. Un échec P2 est plus coûteux qu'un échec P5 ; on attaque dans l'ordre où l'on accepte de céder.

### G — Gate de soundness préalable (bloquant)

Le gate SEF-8 a déjà classé P2/P5 en PROXY et P3a en INSTANCIÉE. **Ce gate-ci ne ré-audite pas : il construit les oracles non-proxy** dont SEF-8 a démontré l'absence, et statue sur leur recevabilité **avant** que les axes ne démarrent.

**G produit, pour chaque axe, l'artefact suivant :**

- **G-P2** — un oracle de rollback qui **falsifie autre chose qu'une tautologie**. Le « hash identique » de SEF-2 est trivial sous content-addressing ; l'oracle adversarial doit cibler la **cohérence de la chaîne** sous conditions adverses (la propriété P-δ de S7 — « la prochaine action après rollback reprend depuis l'état restauré » — est la seule non-triviale ; c'est elle qu'il faut soumettre à l'adversaire, pas P-α). Critère de recevabilité G-P2 : l'oracle distingue un rollback *correct* d'un rollback *qui rend un état déchiré / un parent pendant* — sinon il reste un proxy.
- **G-P3** — trois oracles **distincts** correspondant aux trois dimensions attaquées (D-P3 ci-dessous) : latence, complétude, intégrité. Critère de recevabilité : un oracle de latence ne peut pas falsifier une perte de complétude, et un oracle de complétude ne peut pas falsifier une corruption d'intégrité (même piège que L88 : un compteur d'événements ne falsifie pas un défaut du log lui-même).
- **G-P5** — **statuer d'abord sur la question préalable** (voir D-P5 ci-dessous) : faut-il faire entrer la sortie LLM dans `state_bytes` ? Tant que cette question n'est pas tranchée, **aucun oracle P5 n'est constructible** qui ne soit pas un proxy. G-P5 est donc une **décision de design**, pas une plomberie. Si la réponse est « non », l'axe A-P5 est **clos sans code** (la dette d'oracle #3 reste dormante, statut inchangé ADR-0051 §D5 / ADR-0052 §Clôture). Si « oui », G-P5 produit la spec de l'oracle SEF-6-bis.

**Critère de sortie du gate (bloquant) :** chaque axe dispose d'un oracle non-proxy recevable **ou** d'une décision motivée de non-ouverture. Tant que ce n'est pas écrit, aucun axe ne démarre. Le gate est gratuit (analyse + décision, pas de harness lourd) — il invalide ou recadre les axes en amont.

### D-P2 — Axe A-P2 : rollback adversarial

**Réponse à la question « SEF-2 est-il proxy ou instancié ? » : PROXY**, pour trois raisons cumulées — (1) le hash identique est trivial par construction ; (2) chaîne unique, payload déterministe par index, mono-agent ; (3) la borne ≤ 100 ms est mesurée à un N fixe sur cache chaud (L20 : p95 = 99 µs, marge ×1000 — la borne n'est jamais approchée, donc jamais falsifiée).

**Vecteurs adversariaux retenus (par dimension falsifiée) :**

| Vecteur | Dimension | Le système ÉCHOUE si… |
|---------|-----------|------------------------|
| V2.1 — Longue chaîne (N ≫ 1000, depth maximal avant commit barrier) | **Borne murale** | la durée de rollback dépasse 100 ms à la profondeur que l'adversaire pousse (N borné par le commit barrier — quel est ce N max réel ?) |
| V2.2 — Rollback d'un état déjà rollbacké (rollback²) | **Cohérence chaîne** | le second rollback rend un état incohérent, ou repointe vers un `SnapshotHeader` orphelin/pendant |
| V2.3 — Rollback concurrent (N actions en vol pendant le rollback) | **Atomicité** | une action concurrente s'intercale et produit un état mixte (post-rollback partiellement écrasé) ou un parent pendant |
| V2.4 — Rollback sous flood d'actions simultanées (interaction scheduler) | **Liveness conditionnelle / atomicité** | le rollback est famine-é indéfiniment, OU complété sur un tip qui a bougé |

**Critère go/no-go A-P2 :**
- **GO** si G-P2 a produit un oracle qui distingue un rollback correct d'un état déchiré (pas le hash trivial), ET si au moins V2.2/V2.3 sont constructibles sur Linux PoC sans simulation (L32).
- **NO-GO** si l'oracle reste un proxy (hash trivial) ou si les vecteurs concurrents exigent une infra absente. V2.1 (borne murale longue chaîne) est **conditionné** à la connaissance du N max réel imposé par le commit barrier : si N est borné à ~100–1000 par construction, V2.1 ne falsifie rien de plus que L20 et tombe (non-objectif).

**Sur la borne ≤ 100 ms — est-elle falsifiable adversarialement ?** Oui, **uniquement** si l'adversaire peut pousser `depth` (N depuis le dernier commit barrier) au-delà de ce que le commit barrier autorise. La question préalable est donc : **quel est le N max entre deux commit barriers ?** Si le commit barrier est forcé périodiquement (borne dure), la chaîne ne dépasse jamais une profondeur où 100 ms est en danger (L20 donne ~0,72 µs/saut → 100 ms ≈ 139 000 sauts). **Falsifier la borne exige de démontrer un régime où depth ≥ ~10⁵** — ce qui est soit impossible par construction (commit barrier borne N), soit un vecteur réel à instruire. **Cette question est bloquante pour A-P2** (cf. Questions bloquantes).

### D-P3 — Axe A-P3 : traçabilité causale adversariale

**Réponse à « quels vecteurs adversariaux ? » — décomposés rigoureusement en trois dimensions, avec trois oracles distincts (G-P3) :**

| Vecteur | Dimension attaquée | Oracle | Le système ÉCHOUE si… |
|---------|-------------------|--------|------------------------|
| V3.1 — Flood de lookups concurrents | **Latence** | latence p99 sous charge | p99 dépasse la borne P3a (10 ms) **sous write concurrent** — régime hors P3a (qui est read-only static), proche P3c |
| V3.2 — Write concurrent massif pendant lookups | **Latence / complétude** | p99 + complétude | p99 explose OU une entrée fraîchement appended est introuvable par `get` (fenêtre de visibilité) |
| V3.3 — `action_id` forgé (collision / préfixe / hash bidon) | **Intégrité** | vérification content-addressed | `get(action_id_forgé)` retourne une entrée dont `entry.action_id() != action_id` (intégrité SHA-256 brisée) — c'est SEF-5 P-β étendu à l'adversaire |
| V3.4 — DAG avec cycle injecté (`parent_ids` circulaire) | **Intégrité structurelle** | B-light bloque-t-il ? | l'insertion d'un cycle est **acceptée** (le DAG n'est plus acyclique), OU une reconstruction de chaîne boucle indéfiniment |
| V3.5 — Log 10⁸ entrées + writes concurrents | **Latence (P3c réel)** | p99 multi-agent | dépasse les bornes P3c (≤ 50 ms à N=8, ≤ 100 ms à N=32) — **mais P3c n'a jamais été implémentée ni son workload formalisé** |

**Distinction obligatoire (piège L88) :** un oracle de latence (V3.1/V3.2/V3.5) ne falsifie **pas** la complétude ni l'intégrité ; un oracle d'intégrité (V3.3/V3.4) ne dit rien sur la latence. Ne jamais présenter « la latence dégrade sous flood » comme « la traçabilité est cassée ». La complétude (toute action émise est retrouvable) et l'intégrité (l'entrée retournée est authentiquement celle demandée, le DAG reste acyclique) sont des propriétés de **correction**, pas de performance.

**Critère go/no-go A-P3 :**
- **GO sur V3.3 + V3.4 (intégrité)** : constructibles sans infra (forge d'`action_id`, injection de `parent_ids`), oracle = vérification content-addressed déjà présente (SEF-5 P-β) à étendre adversarialement. Priorité haute — c'est la dimension *correction*, non couverte par P3a/P3b/P3c.
- **GO conditionné sur V3.1/V3.2 (latence sous concurrence)** : recevable seulement si le régime cache est honnête (drop_caches, L32) ET si le workload de write concurrent est défini. Sinon NO-GO (mesure une file d'attente, pas une dégradation).
- **NO-GO sur V3.5 (P3c)** : P3c est **réservée, jamais implémentée**, et son **critère de déclenchement n'est pas atteint** (workload multi-agent non formalisé, `benchmarks/reference-workload.md §W1-access` Modèles A&B non déclarés — cf. spec §P3c). Ouvrir V3.5 reviendrait à instancier P3c sans son préalable. **Hors périmètre de cette campagne** (renvoyé au déclencheur P3c existant).

**B-light et les cycles (V3.4) — question de fait à instruire au gate :** le moteur d'index B3 (ADR-0042, voir `decisions/b3-storage-research.md`) valide-t-il l'acyclicité du DAG à l'insertion, ou accepte-t-il un `parent_ids` arbitraire ? Si l'acyclicité n'est pas enforced, V3.4 falsifie une propriété structurelle de P3 (la traçabilité *causale* suppose un DAG, pas un graphe quelconque). **Cette question est bloquante pour V3.4** et relève d'une vérification de code au gate (consulter l'agent `redb` pour le comportement du backend).

### D-P5 — Axe A-P5 : déterminisme avec oracle non-trivial

**Réponse directe à la question posée :** l'exercice de P5 via l'oracle SEF-6-bis **n'est PAS utile tant que la sortie LLM n'entre pas dans `state_bytes` par construction**. ADR-0052 §Clôture sous-axe B l'établit comme un **constat de fait** (audit `actor.rs:1267-1317`) : `data_hash = put_block([agent_id | seq | zéros])` ne dépend que de `(agent_id, seq)` ; la sortie LLM (`InferResponse.text`) n'entre dans **aucun** champ de la préimage. Un SEF-6-bis branché sur un agent appelant `agent_infer` produirait un hash de transition identique run-à-run **quelle que soit la réponse du modèle** — donc un PASS trivial, exactement le proxy L87 qu'on veut éliminer.

**Donc la décision est : trancher la question de design AVANT tout code (G-P5).**

> **Question préalable bloquante (G-P5) : faut-il faire entrer une primitive non-déterministe dans la préimage `commit_barrier` ?**
>
> Deux branches :
>
> - **Branche NON (statu quo).** L'état hashé reste `(agent_id, seq)` ; la sortie LLM est un **canal observable hors-état** (émis via `emit`, jamais dans `data_hash`). Dans ce cadre, P5 (déterminisme de *transition d'état*) est **tenu trivialement et correctement** : la transition ne dépend pas du LLM, donc elle est déterministe par construction, et SEF-6 actuel suffit à le démontrer. **A-P5 est clos sans code.** La dette d'oracle #3 reste dormante (ADR-0051 §D5), son déclencheur (campagne dédiée OU modif `commit_barrier`) non atteint. C'est la position cohérente avec spec §P5 (« déterminisme de la *transition*, pas de la sortie sémantique ») et avec ADR-0052.
>
> - **Branche OUI (changement de modèle).** On élargit la préimage `state_bytes` pour qu'une valeur dérivée d'une primitive non-déterministe (sortie LLM, entropie, wall-clock) entre dans `data_hash`. **C'est un changement de code structurant** (ADR-0052 §Clôture), pas un agent de référence. Alors — et seulement alors — P5 devient **non-trivial** : il faut que la primitive passe par S6 (substituable en replay) pour que deux runs avec la même substitution produisent le même hash. SEF-6-bis aurait un sens : il falsifierait le système si une primitive non-déterministe entre dans l'état **sans** passer par S6 (trou dans S1/S6). C'est l'oracle non-proxy que SEF-8 réclamait pour P5.

**Critère go/no-go A-P5 :**
- **GO** uniquement si G-P5 tranche **Branche OUI** — c.-à-d. si une décision architecturale (futur ADR) établit que faire entrer la sortie LLM (ou une autre primitive non-déterministe) dans l'état hashé apporte une propriété visée. **Cette décision n'est pas acquise** et n'est pas du ressort de la présente campagne : la campagne ne *crée* pas un besoin de design pour se donner une cible.
- **NO-GO (défaut)** : Branche NON. P5 est correctement tenu trivialement parce que le modèle exclut le LLM de l'état par construction. **Forcer le LLM dans `state_bytes` uniquement pour avoir un oracle P5 non-trivial serait du « tiré par la propreté »** (ADR-0049 §D1, ADR-0051 §D4) : on modifierait l'architecture pour se donner une cible de test, inversion de la discipline. A-P5 est donc **clos sans code par défaut**, et l'ADR le trace comme tel.

**Ce que A-P5 falsifie réellement, s'il est ouvert (Branche OUI) :** non pas « le LLM est déterministe » (faux et hors-sujet — spec/08 §0.1, non-objectif LLM ADR-0050 §D6), mais « toute source de non-déterminisme entrant dans l'état passe par une primitive S6 substituable ». L'oracle est : injecter une primitive non-déterministe **hors S6** et vérifier que le replay diverge (le système doit alors soit détecter, soit la propriété est non-tenue). Sans S1 satisfait, ce contournement est indétectable (spec §P5) — d'où le couplage S1/S6.

### D-final — Ordre d'exécution et conditionnalité

```
G (gate, bloquant)
 ├─ G-P2 : oracle non-trivial rollback        → débloque A-P2
 ├─ G-P3 : 3 oracles (latence/complétude/intég) → débloque A-P3
 └─ G-P5 : DÉCISION Branche OUI/NON            → ouvre ou clôt A-P5
            │
            ▼
A-P2 (rollback adversarial)   [si G-P2 recevable]
            │
            ▼
A-P3 (traçabilité adversariale) [V3.3/V3.4 prioritaires ; V3.1/V3.2 conditionnés ; V3.5 hors scope]
            │
            ▼
A-P5  [OUVERT seulement si G-P5 = Branche OUI ; sinon CLOS sans code]
```

---

## Périmètre (non-objectifs explicites)

- **P3c (multi-agent concurrent, V3.5)** : hors scope. Réservée, jamais implémentée, workload multi-agent non formalisé (spec §P3c). Renvoyée à son déclencheur existant (P3b passée + Modèles A&B déclarés dans `reference-workload.md`).
- **P3-range (fenêtre causale)** : hors scope. Placeholder non-falsifiable (spec §P3-range), index secondaire `agent_ts` non promu.
- **Forcer le LLM dans `state_bytes` pour créer une cible P5** : non-objectif explicite (« tiré par la propreté », ADR-0049 §D1 / ADR-0051 §D4). La Branche OUI de G-P5 n'est ouverte que par une décision de design *indépendante*, pas par le besoin de tester.
- **Frontière LLM (bonté sémantique de la sortie)** : non-objectif, inchangé (spec/08 §0.1, ADR-0050 §D6). P5 porte sur le déterminisme de transition, jamais sur la qualité de la décision LLM.
- **Power-loss / cache-loss réel** : hors scope infra (mur média réel, groupe de dette #8 / D-P3a / β-seL4, ADR-0051 §D4). Les vecteurs concurrents (V2.3, V3.2) testent la concurrence *runtime*, pas le crash média. Tout vecteur exigeant invalidation de cache est traité comme **sévérité construite**, jamais comme **occurrence** (L89).
- **Borne murale P2 à profondeur réaliste (V2.1)** : conditionnée à la démonstration d'un régime depth ≥ ~10⁵ ; sinon non-objectif (L20 couvre déjà la borne à depth ≤ 1000).
- **Transférabilité Linux → seL4** : aucun verdict de cette campagne ne transfère (garde-fou ADR-0050 §D7). Tout verdict nomme « Linux PoC, isolation/concurrence logicielle runtime ».

---

## Gate de soundness préalable : oracles à auditer et critère de recevabilité

| Oracle à produire | Cible | Critère de recevabilité (sortie du gate) |
|-------------------|-------|-------------------------------------------|
| **G-P2** | rollback non-trivial | Distingue rollback correct d'un état déchiré / parent pendant ; ne repose PAS sur le « hash identique » trivial (S7 P-α). S'appuie sur P-δ (reprise post-rollback) étendue à l'adversaire. |
| **G-P3-latence** | p99 sous concurrence | Régime cache honnête (drop_caches, L32) ; workload write concurrent défini. Ne falsifie QUE la latence. |
| **G-P3-complétude** | retrouvabilité | Toute action émise (témoin out-of-band des `append`) est comparée à `get` ; oracle hors du log lui-même (piège L88). |
| **G-P3-intégrité** | content-addressed + DAG acyclique | Étend SEF-5 P-β ; vérifie `entry.action_id() == id` sous forge, ET acyclicité du DAG à l'insertion (vérifier le comportement B-light/redb au code — agent `redb`). |
| **G-P5** | DÉCISION, pas oracle | Tranche Branche OUI/NON. Si NON : A-P5 clos, dette #3 dormante inchangée. Si OUI : exige un ADR de design préalable justifiant l'entrée d'une primitive dans `state_bytes` par une propriété visée. |

**Critère de sortie global du gate :** chaque axe a un oracle non-proxy recevable OU une décision motivée de non-ouverture. SEF-8 a déjà fait l'audit *diagnostique* (P2/P5 = PROXY) ; ce gate-ci fait le travail *constructif* (produire le non-proxy ou clore). Tant qu'un axe n'a ni oracle recevable ni clôture motivée, il ne démarre pas.

---

## Conséquences

### TODO.md — entrées à créer (section « Campagne adversariale P2/P3/P5 »)

- `[ ]` **G — Gate de soundness P2/P3/P5** (bloquant) : produire G-P2, G-P3 (×3), G-P5 (décision). Renvoi ADR-0053 §G. Critère de sortie : oracle non-proxy OU clôture motivée par axe.
- `[ ]` **G-P5 (décision Branche OUI/NON)** — sous-tâche prioritaire du gate : trancher si une primitive non-déterministe doit entrer dans `state_bytes`. Défaut = NON (A-P5 clos sans code, dette #3 dormante inchangée, ADR-0051 §D5). OUI exige un ADR de design préalable.
- `[ ]` **A-P2 — rollback adversarial** : V2.2 (rollback²), V2.3 (rollback concurrent), V2.4 (rollback sous flood). V2.1 (longue chaîne) conditionné à la question N max (bloquante). Renvoi §D-P2.
- `[ ]` **A-P3 — traçabilité adversariale** : V3.3 (`action_id` forgé) + V3.4 (DAG cyclique / B-light) **prioritaires (intégrité)** ; V3.1/V3.2 (latence sous concurrence) conditionnés au régime cache + workload. V3.5 (P3c) **hors scope**. Renvoi §D-P3.
- `[ ]` **A-P5** — **conditionnel** : ouvert seulement si G-P5 = Branche OUI. Sinon clos sans code (cocher avec renvoi §D-P5).
- Famille de scénarios : `SEF-11` (gate constructif), `SEF-12` (A-P2), `SEF-13` (A-P3), `SEF-14` (A-P5, conditionnel) — ou `S15+` selon ADR-0021.

### Cohérence ADR

- **ADR-0050 §D6** : cet ADR **instruit** la campagne P2/P3/P5 différée (« différée non annulée »). Ne rouvre pas l'axe plafonds (condition C2/hardware toujours ouverte).
- **ADR-0051 §D5** : respecté. La dette d'oracle P5 #3 reste dormante par défaut (Branche NON) ; cet ADR ne l'amende pas, il **statue sur son déclencheur** (G-P5).
- **ADR-0052 §Clôture sous-axe B** : intégré comme constat de fait — le vrai déclencheur P5 est la modif `commit_barrier`, pas `kv_store`. Repris en §D-P5.
- **spec/02 §P2, §P3c, §P5** : non amendés — la campagne *exerce* (ou clôt) ; toute modif §P5 (Branche OUI) passerait par un ADR de design dédié, hors de cet ADR de cadrage.
- **ADR-0049 §D1 / ADR-0051 §D4** : invoqués — interdiction du « tiré par la propreté » justifie le NO-GO par défaut d'A-P5.
- **lab/LESSONS.md** : L87 (proxies P2/P5), L88 (oracle hors-bande, dimension de sécurité), L89 (occurrence vs sévérité construite), L32 (cache honnête), L68 (critère avant code) — tous opérants.

---

## Questions tranchées dans cet ADR (étaient bloquantes)

1. **SEF-2 proxy ou instancié ?** → PROXY (hash trivial + chaîne unique + borne jamais approchée). §D-P2.
2. **La borne ≤ 100 ms est-elle falsifiable adversarialement ?** → Oui, mais **seulement** si depth ≥ ~10⁵ est atteignable ; sinon non (commit barrier borne N). Question N max reste à instruire (bloquante A-P2).
3. **Quels vecteurs P3 ?** → V3.3/V3.4 (intégrité, prioritaires), V3.1/V3.2 (latence, conditionnés), V3.5/P3c (hors scope). Trois dimensions, trois oracles. §D-P3.
4. **L'oracle P5 SEF-6-bis est-il utile si le LLM n'entre pas dans `state_bytes` ?** → NON. Décision de design (G-P5) requise avant tout code. Défaut = Branche NON, A-P5 clos sans code. §D-P5.
5. **Ordre ?** → G → A-P2 → A-P3 → A-P5 (conditionnel), suivant ADR-0001 (P2 ≻ P3 ≻ P5). §D-final.

---

## Questions encore ouvertes (à trancher au gate, pas dans ce cadrage)

- **N max entre deux commit barriers** (bloquante A-P2 / V2.1) : le commit barrier est-il forcé périodiquement avec une borne dure sur depth ? Vérification de code + éventuel ADR. Si N borné petit, V2.1 tombe.
- **B-light / redb valide-t-il l'acyclicité du DAG à l'insertion ?** (bloquante A-P3 / V3.4) : vérification de code (agent `redb`, ADR-0042, `decisions/b3-storage-research.md`). Si non enforced, V3.4 falsifie une propriété structurelle de P3.
- **G-P5 : existe-t-il une propriété visée qui justifie de faire entrer une primitive non-déterministe dans `state_bytes` ?** Si non (défaut attendu), A-P5 reste clos. Décision d'ADR de design indépendant, jamais motivée par le besoin de tester.

---

## Références

- `decisions/0050-campagne-mise-a-lepreuve.md` §D2 (gate soundness), §D6 (P2/P3/P5 différés), §D7 (non-transférabilité) — méthode de référence
- `decisions/0051-cloture-campagne-tri-findings.md` §D5 (dette oracle P5 #3 dormante), §D4 (interdiction « tiré par la propreté »)
- `decisions/0052-scope-phase-10-inference-reelle.md` §Clôture sous-axe B (vrai déclencheur P5 = préimage `commit_barrier`, pas `kv_store`)
- `decisions/0049-cloture-poc-sel4.md` §D1 (refus du « tiré par la propreté »)
- `decisions/0001-priorite-proprietes.md` — ordre P4 ≻ P2 ≻ P3 ≻ P6 ≻ P5 ≻ P1 (D0)
- `decisions/0042-voie-b3-moteur-index.md`, `decisions/b3-storage-research.md` — moteur d'index B3 (V3.4 acyclicité)
- `spec/02-properties.md` §P2 (O(depth), borne 100 ms), §P3a/b/c (portées), §P3-range (placeholder), §P5 (garantie conditionnelle S1/S6)
- `spec/08-modele-menace.md` §0.1 — happened-before vs sémantique (non-objectif LLM)
- `poc/scenarios/S7-rollback-equivalence/README.md` (SEF-2, hash trivial), `S14-causal-lookup/README.md` (SEF-5/P3a, read-only static), `S8-determinism/README.md` (SEF-6, agent trivial)
- `poc/runtime/src/actor.rs:1267-1317` (préimage `commit_barrier` = `[agent_id|seq|zéros]`), `poc/runtime/src/clock.rs:93-99` (`LogicalClock`)
- `lab/LESSONS.md` L87 (P2/P5 = PROXY), L88 (oracle hors-bande), L89 (occurrence vs sévérité), L32 (cache honnête), L68 (critère avant code)
