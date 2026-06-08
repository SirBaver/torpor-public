# Spécification 08 — Modèle de menace

**Version :** 1.6  
**Date :** 2026-06-03  
**Statut :** Brouillon révisé — §2/T1 updated, T12 added (confused-deputy SEF-9, correctif ADR-0051 §D2), §5 updated (ADR-0051 campagne close). §3 + §8 mis à jour : campagne B close, limites structurelles documentées.

---

## 0. TCB — Trusted Computing Base déclarée

Toutes les défenses listées dans ce document présupposent l'intégrité des composants suivants :

| Composant | Rôle dans la défense |
|-----------|---------------------|
| **Wasmtime** (sandboxing WASM) | Isolation mémoire inter-agents (`Store` par agent), impossibilité d'exécuter du code natif non sanctionné |
| **RocksDB** | Durabilité du CausalLog et du ContentStore, sémantique de `put_cf` et `append` |
| **Runtime Rust** (`poc/runtime/`) | Implémentation des host functions, enforcement des capabilities, séquence log-avant-mutation |
| **Kernel Linux** | Isolation de processus, accès `/proc`, scheduling |

Un adversaire ayant compromis un composant du TCB (bug Wasmtime permettant l'évasion du sandbox, race condition dans RocksDB, etc.) invalide les défenses correspondantes. Ces scénarios sont **hors périmètre du PoC** mais doivent être documentés avant tout déploiement multi-tenant.

### TCB cible seL4 (Phase 8+)

Lors de la transition vers seL4 (ADR-0037/0038), le TCB Linux est remplacé par :

| Composant | Rôle dans la défense |
|-----------|---------------------|
| **seL4** (kernel vérifié formellement) | Isolation de processus, médiation des capabilities, IPC |
| **Runtime Rust** (Wasmtime + executor async no_std) | Sandbox WASM (S1b), host functions, enforcement capabilities |
| **Serveur de stockage** (Rust, processus seL4 séparé) | Durabilité du journal causal et du ContentStore, atomicité P6 |

---

## 0.2 — Politique « C dans le TCB » (seL4, Phase 9+)

La règle historique « Rust pur dans le TCB » (ADR-0037 §3) est trop binaire pour la Phase 9 (drivers hardware). Reformulation opérationnelle :

**Option retenue : α** — *Les composants isolés dans des processus seL4 distincts avec une interface IPC typée ne font pas partie du TCB Rust, mais font partie du TCB OS.*

Conséquences :
- **TCB Rust** : composants pouvant corrompre arbitrairement l'état d'un agent sans passer par seL4 IPC. Comprend : runtime Wasmtime, executor, host functions, serveur de stockage. Doit être en Rust.
- **TCB OS** : composants dont le comportement incorrect compromet une propriété système globale (P1–P6). Comprend le TCB Rust + seL4 (C vérifié formellement) + drivers seL4 userspace (C ou Rust selon disponibilité). seL4 isole les drivers C du TCB Rust via les capabilities.
- **Conséquence Phase 9** : un driver block sDDF/blk (C) isolé dans un processus seL4 est dans le TCB OS mais pas dans le TCB Rust. Acceptable si : (a) sa surface d'interface est une ABI IPC typée et documentée, (b) son LOC est auditable (cible < 5 KLOC C).

**Ce qui reste interdit :** du C dans le processus runtime Wasmtime ou dans les host functions (le TCB Rust). seL4 est la frontière.

**Question ouverte Phase 9 :** statut de vérification de sDDF/blk [Heiser et al. 2024, « Towards Verified I/O Frameworks »] — si le driver est formellement vérifié (Cogent ou équivalent), il rejoint seL4 dans la catégorie « C formellement vérifié ». À documenter dans l'ADR driver (Phase 9).

---

## 0.1 — Contrat sémantique du graphe causal (CausalLog DAG)

Le `CausalLog` est un **Lamport-DAG enrichi de citations explicites** [Lamport 1978,
"Time, Clocks, and the Ordering of Events"]. Chaque `LogEntry.parent_ids` contient le
parent séquentiel (dernier `action_id` émis par l'agent) et les causes supplémentaires
déclarées via `agent_add_cause`.

**Ce que le graphe garantit (sous B-light, ADR-0036) :**

- Chaque `action_id` référencé dans `parent_ids` existait dans le `CausalLog` au moment
  du `commit_barrier` qui a créé cette entrée (vérification d'existence O(1) en mémoire
  RocksDB/memtable).
- L'agent émetteur a explicitement invoqué `agent_add_cause(action_id)` pour déclarer
  cette cause.
- Le nombre de causes supplémentaires est borné à `MAX_EXTRA_CAUSES = 16`.

**Ce que le graphe ne garantit pas :**

- **Connaissance effective** : que l'agent émetteur avait connaissance du *contenu* de
  l'action citée au moment de sa décision. L'agent peut citer une action dont il n'a
  jamais lu le payload.
- **Fenêtre cognitive** : que la citation provient de la fenêtre de contexte LLM courante
  de l'agent. Un agent peut citer ses propres actions d'une session antérieure ou d'une
  `action_id` transmise par un canal applicatif.
- **Causalité sémantique** : que l'action citée a réellement influencé la décision. La
  citation est un acte volontaire de l'agent, pas une inférence du runtime sur la
  causalité effective.
- **Ordering total** : les citations croisées entre agents créent un partial order, pas
  un ordre total. Deux entrées non reliées dans le DAG ne sont pas ordonnées.

**Conséquence pour les consommateurs (superviseur, reconstructeur) :**

Tout raisonnement du type *"B a cité A, donc B avait connaissance de A"* ou *"B a réagi
à A"* est **hors des garanties du système**. Le DAG causal prouve l'ordering de référence
(A existait avant que B le cite) — pas l'influence sémantique ni la fenêtre cognitive.

Cette propriété est structurelle : aucun runtime logiciel ne peut certifier le contenu
de la fenêtre de contexte d'un LLM externe. La distinction entre *happened-before*
(observable, garanti) et *knowledge* (non observable, non garanti) est fondamentale en
informatique distribuée [Halpern & Moses 1990, "Knowledge and Common Knowledge in a
Distributed Environment"]. Le DAG du `CausalLog` implémente le premier, pas le second.

**Lien avec T9 (§4) :** T9 (auto-citation hors session) n'est pas un exploit contre le
runtime — c'est une conséquence directe de cette propriété. Il est documenté ici comme
propriété du modèle, pas comme dette d'implémentation.

---

## 1. Adversaire

**Un agent compromis** : un agent dont le code WASM exécute une logique malveillante —
injectée par une sortie LLM adversariale, une supply-chain compromise du module `.wasm`,
ou un agent légitime dont le comportement a dérivé au-delà de ses intentions initiales.

L'adversaire peut : exécuter du WASM arbitraire dans son sandbox Wasmtime, appeler
toute host function qu'il importe dans son module WASM (voir §1.1), observer le timing
de ses propres appels, et émettre des données dans le log causal.

L'adversaire **ne peut pas** : exécuter du code natif sur l'hôte, accéder à la mémoire
WASM d'un autre agent (isolation Wasmtime par `Store`), appeler une host function
non déclarée dans la section `(import ...)` de son module.

### 1.1 — Gate d'accès aux host functions

**Seules `agent_store_get`, `agent_store_put`, et `agent_check_cap` sont capability-gated** :
elles refusent l'appel si `CapabilityStore::check` échoue. Toutes les autres host
functions exposées (`commit_barrier`, `emit`, `agent_self_rollback`, `agent_introspect`,
`agent_add_cause`, `agent_terminate`, `agent_checkpoint`, `agent_session_info`,
`agent_request_validation`, `agent_get_verdict`) sont accessibles à tout agent qui
les importe dans son module. Leur seule protection est contractuelle (borne sur `depth`,
taille de payload) — pas un gate d'autorisation.

La phrase "l'adversaire peut appeler toute host function pour laquelle il a une capability"
serait trompeuse : pour la majorité des host functions, l'import suffit.

---

## 2. Périmètre — ce qui est dans le modèle

### T1 — Bypass de capability

**Description :** L'agent tente d'accéder à une ressource (`agent_store_get`,
`agent_store_put`) sans capability valide ou avec une capability insuffisante.

**Mécanisme de défense :** `CapabilityStore::check` appelé dans chaque host function
capability-gated avant toute lecture/écriture. Refus → `CapabilityDenied (0x14)` émis
dans le log causal. Rate-limiting intégré par resource (≤ 32 resources distinctes/fenêtre,
correctif ADR-0051 §D2) pour préserver l'attribution d'audit même sous flood (SEF-9).

**Couverture :** Testée. SEF-3 (scénario S9) : sur les classes d'accès couvertes par le
scénario (accès `agent_store_get/put` sans capability, avec capability insuffisante,
après révocation), 100 % des accès refusés, 100 % des refus loggués. Voir
`spec/02-properties.md §P4`. Note : "100 %" est relatif aux classes C1–C4 testées dans
S9, pas à l'espace infini des accès non autorisés concevables (propriété structurelle
de *complete mediation* [Saltzer & Schroeder 1975] — non prouvée formellement).

**Résidu :** Les host functions non-gated listées en §1.1 sont accessibles sans
capability. Leur gate est uniquement la borne contractuelle.

---

### T2 — (Prospectif) Oracle de déduplication ContentStore

**Statut dans le PoC actuel : Non substrat — risque prospectif uniquement.**

**Pourquoi T2 n'a pas de substrat dans le PoC :**

(a) `ContentStore::put_block` (`poc/store/src/lib.rs:97-102`) appelle
`self.db.put_cf(cf, hash, data)` **inconditionnellement**. RocksDB écrit dans le memtable
et le WAL que la clé soit présente ou non — la déduplication au sens LSM se produit à
la compaction, pas au `put`. Il n'existe pas de fast-path "clé déjà présente → no-op"
observable par timing à ce niveau. La phrase "l'écriture est un no-op dans RocksDB
(clé déjà présente)" qui figurait en version 1.0 était techniquement incorrecte.

(b) Le contenu écrit par `commit_barrier` dans `put_block` est
`agent_id (16 bytes) || seq (8 bytes LE) || 0x00...` (dérivé de `actor.rs:1186-1193`).
Ce contenu est **unique par `(agent_id, seq)`** par construction : aucun autre agent ne
peut produire un bloc de même contenu. Il n'y a donc pas de déduplication inter-agents
sur ce chemin.

(c) Le seul chemin où un agent injecte du contenu de son choix est `agent_store_put`,
qui écrit dans un `HashMap` en mémoire **par-agent** (`actor.rs:633`) — strictement
isolé, hors ContentStore partagé.

**Risque prospectif :** Si une future primitive `agent_blob_put(content) → hash` exposait
du contenu arbitraire fourni par l'agent dans le ContentStore partagé, l'oracle de
Harnik–Pinkas–Shulman-Peleg [2010] s'appliquerait. À documentér à la conception de
toute nouvelle primitive ContentStore ouverte. Référence : [Harnik, Pinkas,
Shulman-Peleg 2010, "Side Channels in Cloud Services: Deduplication in Cloud Storage"].

---

### T3 — Canal temporel C2-acquire

**Description :** `IoAdmissionQueue::acquire` bloque l'agent appelant jusqu'à ce
qu'un slot soit disponible (`cap_actif` slots simultanés). La latence observable depuis
WASM via `agent_infer` (temps d'attente C1) est un proxy indirect de la charge partagée.
Un agent malveillant peut mesurer ses propres latences d'`agent_infer` pour inférer
la charge du pool C1 (InferencePool).

**Note :** En pratique, le WASM agent n'appelle pas directement `IoAdmissionQueue` —
c'est le `Scheduler::deliver` qui l'appelle. La latence de `agent_infer` reste la
surface observable depuis WASM.

**Impact :** Canal de bas débit, bruit élevé (dépend du workload de fond).
Peu exploitable pour exfiltrer des données ; utile pour inférer le nombre d'agents
actifs et leur fréquence d'inférence. La capacité réelle du canal n'a pas été
quantifiée (mesures à faire si multi-tenant visé).

**Couverture :** Non adressée. Mitigation classique : bruit artificiel sur les latences
retournées (coût : dégradation de la réactivité légitime).

---

### T4 — Weaponisation du rollback

**Description :** `agent_self_rollback(depth)` avec `depth ∈ {1, 2, 3}` permet à
l'agent de supprimer jusqu'à 3 actions de son propre historique local (état WASM
restauré, pointeur snapshot ContentStore reculé). L'adversaire s'en sert pour effacer
des traces d'activité indésirable.

**Mécanisme de défense :**

- Depth borné à 3 (ADR-0019 §D3). Pas de rollback epoch.
- L'événement `SelfRollback (0x07)` est émis dans le log causal **avant la mutation
  de l'état agent en mémoire** (`actor.rs:1319-1385` : `log.append(&entry)` précède
  `self.last_snapshot = target_snap` dans le même match arm). La sémantique correcte
  est : *l'entrée log précède la mise à jour des pointeurs in-process* — pas que
  l'effet ContentStore (écritures futures) soit déjà annulé au moment du log.
- Les snapshots ContentStore des états rollback-cibles restent dans la DB ; seul le
  pointeur `last_snapshot` de l'agent est reculé.

**Limites et qualifications du mécanisme :**

(a) **"Append-only" est une propriété d'API, pas cryptographique.** `CausalLog::append`
écrit dans RocksDB. RocksDB autorise `delete_cf`, `DeleteRange`, l'effacement de fichiers
SST. Il n'y a ni Merkle chaining (style Certificate Transparency [RFC 6962]), ni
signature, ni stockage WORM. L'adversaire **WASM** ne dispose d'aucune host function
de suppression — c'est la vraie défense. Un adversaire avec des privilèges hôte
pourrait purger l'audit trail, mais cela est hors périmètre (opérateur trusted, §3).

(b) **Fenêtre fsync.** `CausalLog::append` est non-fsync (`causal-log/src/lib.rs:365-371`).
Sur crash process du runtime juste après l'append du `SelfRollback (0x07)` et avant
tout flush WAL, l'entrée peut être perdue. Un adversaire capable de provoquer un crash
hôte dans cette fenêtre aurait un effet net : rollback effectif (pointeur mémoire reculé
au moment du crash) + trace potentiellement perdue. Crash délibéré est probablement hors
périmètre (exige un bug Wasmtime ou une faille kernel), mais la fenêtre existe.

(c) **Le SelfRollback est journalisé, pas signalé.** Aucun mécanisme d'alerte ou de
"flag pending review" n'est documenté. Si le superviseur ne lit le log qu'à intervalle
T et que l'agent fait `rollback(3) → 3 nouvelles actions normales` dans cet intervalle,
le superviseur voit le `0x07` mais doit le corréler activement avec le comportement
post-rollback. Ce n'est pas la même propriété que "détection automatique d'abus".

**Impact :** Faible pour un adversaire WASM pur. La trace `0x07` reste dans le log sauf
crash dans la fenêtre fsync. Un superviseur actif peut détecter les patterns anormaux.

**Couverture :** Propriété structurelle (API append-only). Non testée spécifiquement
comme vecteur adversarial — SEF-2 teste la correction du rollback légitime, pas son abus.

---

### T5 — Épuisement du pool d'inférence (DoS C1)

**Description :** Un agent malveillant sature l'`InferencePool` et bloque les autres
agents en `WaitingInference`.

**Mécanisme de défense :** `InferencePool` borné (cap C1). Un `agent_infer` en cours
par agent (run_loop séquentielle). La priorité (Supervisor > Foreground > Batch) avec
promotion anti-famine (ADR-0022) protège les inférences critiques.

**Couverture :** Partielle. La borne dure (cap C1) est testée (S3). Promotion
anti-famine testée (ADR-0022). Non testé : groupe d'agents colludant pour saturer C1.
La collusion reste hors périmètre Phase 7 (§3).

---

### T6 — Forgerie causale via `agent_add_cause` *(criticité haute)*

**Description :** La host function `agent_add_cause(action_id_ptr: i32)` pousse un
`action_id: [u8; 32]` dans `caller.data_mut().pending_extra_causes` sans aucune
validation (`actor.rs:1243-1260`). Ces causes supplémentaires sont sérialisées dans
le `LogEntry` au prochain `commit_barrier` et deviennent partie du graphe causal.

**Exploits concrets :**

(i) **Cause vers un action_id inventé** : l'agent forge un `[u8; 32]` arbitraire.
Le `LogEntry` créé référence un parent qui n'existe pas dans le log. Le reconstructeur
(`poc/reconstruct/`) trouve un parent_id manquant — selon le traitement (ignoré,
erreur, fallback), c'est soit un fuzz contre le reconstructeur, soit une pollution
silencieuse du graphe causal.

(ii) **Cause vers l'action d'un autre agent** : si un `action_id` d'agent A fuite
(via `emit` croisé, canal applicatif, ou `agent_introspect`), un agent B compromis
peut se déclarer causalement postérieur à A. Le superviseur, analysant le graphe,
conclurait que B a réagi à A — conclusion fausse. C'est un *confused deputy* [Hardy 1988]
appliqué à un système causal : le superviseur est trompé sur l'ordering réel.

(iii) **Cycles et paradoxes causaux** : un agent peut forger des cycles ou des
"preuves" d'ordering inexistantes. Si le système de validation ou de rollback exploite
la causalité pour décider des rollbacks en cascade, une forgerie peut forcer des
rollbacks d'agents tiers innocents.

(iv) **DoS mémoire** : aucune borne sur `pending_extra_causes.len()`. Un agent peut
pousser des millions de causes forgées avant un `commit_barrier`, gonflant la mémoire
du runtime et la taille du `LogEntry` sérialisé (et donc du `parent_ids` dans le hash
de l'`action_id`, `actor.rs:1218-1220`).

**Défenses implémentées (ADR-0036, B-light) :**
- `pending_extra_causes.len() >= MAX_EXTRA_CAUSES (16)` → refus immédiat (-2), checké
  avant lecture mémoire.
- `log_ref.get(&action_id)` → refus si action_id inconnu (-3) ou erreur I/O (-4).
- Capability cross-agent non requise pour le PoC mono-tenant (reportée à B-fort,
  déclenché par passage multi-tenant — ADR-0036 §sortie B-fort).

**Impact :** Réduit à moyen. La forgerie de causes vers des action_ids inexistants est
bloquée. La citation d'une action réelle d'un autre agent (dont l'id aurait fuité) reste
possible sans capability cross-agent — acceptable en mono-tenant.

**Couverture :** Implémentée (B-light, `actor.rs:1243-1260`). Tests adversariaux à
ajouter : SEF-7.1 (forgerie refusée), SEF-7.2 (flood rebounded à 16), SEF-7.3
(robustesse reconstructeur sur parent_id manquant).

---

### T7 — DoS mémoire via `pending_extra_causes` non borné

**Description :** Dérivé de T6. Avant tout `commit_barrier`, un agent accumule `N`
causes forgées. L'impact est la mémoire du runtime (vecteur `Vec<[u8;32]>` croissant)
et la taille du `LogEntry` sérialisé dans RocksDB.

**Couverture :** Adressée par ADR-0036 — `MAX_EXTRA_CAUSES = 16`, checké avant lecture
mémoire. Test : SEF-7.2.

---

### T12 — Confused-deputy : rate-limit anti-DoS vs. complétude d'audit (P4 observabilité)

**Description :** Un agent malveillant inonde le runtime avec >100 refus bénins (ex. `agent_store_get` sur des resources sans capability) dans une même fenêtre temporelle (1 s). Le rate-limit anti-DoS du log (`CapabilityDenied 0x14`, max 100 refus/agent/1 s) sature. L'agent profite ensuite de cet état saturé pour tenter un accès malveillant (ex. `agent_store_get("secret")`) — ce refus malveillant est silencié par le rate-limit et **non attribuable** dans le log d'audit, violant la **complétude d'audit** (3ᵉ critère conjonctif de P4) bien que l'**isolation** (capability refusée) tienne.

**Nature du vecteur :** Confused-deputy [Hardy 1988] entre deux mécanismes de défense : le rate-limit prévient un DoS du log (refus bénins massifs) mais devient un **angle mort** de l'audit en masquant une tentative malveillante noyée dans le flood.

**Exploits concrets :**

(i) **Flood + masquage.** Agent A, sans aucune capability, exécute `for i in 1..101: agent_store_get("bn")` → 100 premiers refus loggés avec resource `"bn"`, le 101ᵉ agrégé (rate-limited=0x01, resource omise, compteur seul). Agent A exécute alors `agent_store_get("secret")` → refusé, loggé ? Non, silencié. Écart : `"secret"` est une tentative effective (observable via témoin hors-bande dans le test) mais absent du log.

(ii) **Isolation P4-1a reste intacte.** Le refus malveillant retourne `-1` (accès refusé) ; la capability n'est jamais accordée à tort. Aucune escalade d'accès.

(iii) **Audit P4-3 brisée.** Un superviseur lisant le log n'a aucune trace attribuée du refus sur `"secret"` — il ignore que cette tentative a eu lieu. C'est une **perte d'observabilité**, pas d'isolation.

**Défense implémentée avant le correctif :** Rate-limit agrégé par agent uniquement (`cap_id + count`), sans tracking des resources refusées. Sous flood ≥100, les refus supplémentaires sont silencés sans indication de quelle resource était visée.

**Défense implémentée après correction #6 (ADR-0051 §D2, 2026-05-30) :**

Le rate-limit `0x14` agrège désormais **par resource** (`cap_denied_resources`, ensemble ordonné de ≤32 resources distinctes par fenêtre) :
- Une resource **nouvelle** refusée, même sous flood, reste attribuée **avec son identifiant** dans le log, même après agrégation.
- Le compteur total des refus reste borné (anti-DoS du log).
- Un ensemble maximal de 32 resources distinctes est toujours observable — suffisant pour différencier une ressource sensible (`"secret"`) d'un bruit benin (`"bn"`).

**Couverture :** Test adversarial SEF-9 (`poc/scenarios/SEF-9-confused-deputy-audit/VERDICT.md`). Avant correctif : finding confirmé (`masked={"secret"}`, audit masqué). Après correctif #6 : régression-test, masquage levé (`masked={}`). P4 rehaussée : « complétude d'audit jusqu'au rate-limit anti-DoS ; au-delà, attribution préservée pour tout ensemble borné de resources distinctes nouvelles ».

**Impact :** Élevé pour observabilité d'audit, nul pour isolation de capability. Correctif à coût très faible (~5 lignes, O(1) espace par resource nouvelle, couplé à la vague de correction P4 de l'ADR-0051).

---

## 3. Hors périmètre — explicitement exclu

| Vecteur | Raison d'exclusion |
|---------|-------------------|
| **Power-loss / kernel panic** | Documenté ADR-0027. Exige `WriteOptions::sync=true` sur chemin chaud — incompatible avec P3b ≤ 20 ms sans hardware qualifié. |
| **Adversaire réseau externe** | Le PoC n'expose pas de surface réseau. |
| **Opérateur malveillant** | L'opérateur est trusted par construction (PoC mono-tenant). La résistance à l'effacement du CausalLog par l'opérateur est hors périmètre. |
| **Side channels hardware** | Spectre, Meltdown, Rowhammer — hors périmètre OS logiciel. |
| **Injection de prompt LLM** | Le LLM output est traité comme donnée non fiable (`emit_payload`) — l'OS ne l'interprète pas comme code. La responsabilité de valider le sens du payload appartient au superviseur applicatif. |
| **Collusion multi-agents** | Deux agents compromis qui coordonnent leurs appels. Hors périmètre Phase 7 ; à adresser avant déploiement multi-tenant. |
| **Module `.wasm` supply-chain** | L'OS fait confiance à l'intégrité du module au chargement. Hors périmètre PoC. |
| **Évasion sandbox Wasmtime** | Suppose un bug dans Wasmtime. Hors périmètre — dans le TCB. Documenté comme limite structurelle B-1 (campagne B, 2026-06-03 ; `cargo audit` : 15 CVE actifs sur v25, dont 2 critiques 9.0 sandbox escape — RUSTSEC-2026-0096/0095 N/A par config, cf. B-1). |

### Note — limites structurelles substrat (campagne B, close 2026-06-03)

Les vecteurs hors-périmètre liés au TCB Linux ont été formalisés dans la campagne red team substrat. Résultat : 1 limite correctible (B-2 : incohérence bounds-check sur `agent_add_cause` / `agent_check_cap`, **patch appliqué** `actor.rs`), 4 limites structurelles :

| Finding | Classe | Type |
|---------|--------|------|
| B-1 | Evasion sandbox Wasmtime (CVE actifs sur v25, logiciel seul ; classe Cranelift active) | Structurelle |
| B-3 | N agents dans 1 processus Linux (post-evasion = tout le processus) | Structurelle |
| B-4 | W^X via `mprotect()` logiciel (revocable par kernel exploit) | Structurelle |
| B-5 | TCB Linux ~30 MLOC non prouve (LPEs classe active) | Structurelle |

Ces limites ne sont pas adressables par du code dans le PoC Linux. Elles motivent le substrat seL4 (voir `red-team/campagne-B-substrat/SYNTHESE.md` pour l'argument differentiel complet).

---

## 4. Vecteurs additionnels — non adressés (brefs)

Ces vecteurs n'ont pas fait l'objet d'analyse complète mais sont identifiés pour suivi :

- **T8 — Épuisement budget session** : `MAX_ACTIONS_PER_SESSION` (ADR-0012) —
  un agent peut-il provoquer son propre éviction prématurée pour échapper à une
  surveillance basée sur fenêtre de session ?
- **T9 — Auto-citation hors session (DOCUMENTÉ — §0.1)** : un agent peut citer ses
  propres actions d'une session antérieure ou hors fenêtre LLM courante via
  `agent_add_cause`. B-light vérifie l'*existence* de l'`action_id`, pas la fenêtre
  cognitive de l'agent. Ce comportement est une propriété du modèle, non un exploit :
  le DAG causal ne prétend pas garantir la connaissance effective (§0.1). Aucune
  implémentation requise. Un superviseur ne doit pas inférer "B avait connaissance de A"
  depuis `parent_ids` seul.
- **T10 — Empoisonnement du log via `emit` (payload non borné)** : `emit` accepte
  un payload de taille bornée ? — à vérifier. Un adversaire peut empoisonner le log
  avec du contenu coûteux à traiter pour le superviseur ou pour `reconstruct/`.
- **T11 — Flood `agent_request_validation`** : aucun rate-limit visible ; l'agent
  peut saturer la file du superviseur.

---

## 5. Cartographie sur les propriétés système

| Propriété | Vecteur couvert | État |
|-----------|----------------|------|
| P4 — Confinement capabilities | T1 | Testé (SEF-3, classes C1–C4) |
| P2 — Rollback réversible | T4 (abus rollback) | Structurel (API), non testé adversarialement |
| P1 — Auditabilité | T4 (trace rollback) | Propriété API — non cryptographique, dépend fsync |
| P1 — Auditabilité | **T6 (forgerie causale)** | B-light implémenté (ADR-0036), SEF-7.1/7.2/7.3 testés |
| P4 — Observabilité d'audit | **T12 (confused-deputy rate-limit ↔ audit)** | SEF-9 confirmé avant correctif ; correctif #6 implémenté (ADR-0051 §D2), régression-test PASS |
| — | T9 (auto-citation hors session) | Propriété du modèle — documentée §0.1, aucune implémentation requise |
| P1 — Auditabilité | T5 (DoS C1) | Non testé |
| — | T2 (canal déduplication) | Non substrat dans PoC (voir §2/T2) |
| — | T3 (canal C2 timing) | Non adressé |
| — | T7 (DoS mémoire) | Non adressé |

---

## 6. Risques résiduels non adressés (par ordre de criticité)

### R1 — Citation cross-agent sans preuve de visibilité (T6 résiduel)

**Criticité : Faible en mono-tenant. Accepté tel quel pour le PoC.** La forgerie vers
des action_ids inexistants est bloquée (ADR-0036 B-light). Résidu : un agent peut citer
l'action réelle d'un autre agent si l'action_id lui est parvenu par un canal applicatif.
En mono-tenant, ce canal implique une transmission par l'opérateur (trusted), qui est
hors modèle de menace (cf. §1).

**Statut B-fort : dormant.** B-fort (capability cross-agent, ADR-0036 §sortie) n'est pas
instruit dans le PoC. Décision 2026-05-26 : tant que `Runtime` est mono-tenant, B-fort
n'apporte aucune propriété vérifiable supplémentaire ; son coût (typage `Cap<T>`,
refactor des host functions concernées, tests d'isolation) est non justifié. Réveil
conditionné au trigger objectif documenté dans `TODO.md §Sécurité` et ADR-0036 §sortie :
première introduction d'un second `TenantId` dans `Runtime`.

### R2 — Canal timing C2/C1 (T3)

**Criticité : Faible pour PoC mono-tenant.** Bande passante du canal non quantifiée
(mesures à faire si multi-tenant visé). Devient critique en multi-tenant où la fréquence
d'inférence des autres agents est une donnée sensible.

### R3 — Collusion multi-agents (hors périmètre)

**Criticité : À évaluer avant production.** Deux agents compromis peuvent saturer C1
et C2 de façon coordonnée. Atténuation principale : rate-limiting par-agent sur
`agent_infer` (non implémenté).

---

## 7. Questions bloquantes

*Ces questions doivent trouver une réponse avant que ce document soit considéré comme
une spec sérieuse plutôt qu'une note de travail.*

**(a) ~~Modèle d'autorité de `agent_add_cause`~~** : **Résolu — ADR-0036.** Modèle
B-light retenu : vérification d'existence O(1), `MAX_EXTRA_CAUSES = 16`. Capability
cross-agent reportée à passage multi-tenant.

**(b) Définition opérationnelle de "audit trail préservé"** : préservé contre qui
(adversaire WASM seulement ? + opérateur ? + crash process ?), pendant combien de temps,
avec quelle garantie d'intégrité (none / hash chain / signature) ?

**(c) Modèle de non-interférence visé** : sans définir informellement la propriété
(style [Goguen & Meseguer 1982, "Security Policies and Security Models"]), il n'y a
pas de critère pour dire qu'un vecteur T_x est ou n'est pas une violation. T2 a été
identifié comme non-substrat dans le PoC *grâce à une lecture du code* — pas parce
qu'il y avait un critère formel permettant de le réfuter. Définir le modèle même
informellement.

---

## 8. Références

- `red-team/campagne-B-substrat/SYNTHESE.md` — limites structurelles Linux (B-1..B-5), argument seL4 differentiel (2026-06-03)
- ADR-0051 — Clôture campagne adversariale (tri findings, correctif #6 rate-limit par resource)
- ADR-0050 — Campagne mise à l'épreuve, cadrage axes 1a/1b
- ADR-0036 — B-light forgerie causale (T6)
- ADR-0022, ADR-0023 — InferencePool borné, priorité, promotion anti-famine
- ADR-0029 — `scope_covers` capability, émission CapabilityDenied côté runtime
- ADR-0027 — Portée P6 atomicité crash (power-loss exclu)
- ADR-0003 — Cross-agent causality (voir Q.a)
- ADR-0019 §D3 — Borne rollback depth ≤ 3
- `spec/02-properties.md §P4` — Confinement capabilities, complétude d'audit (amendement ADR-0051)
- `poc/scenarios/S9-capability-isolation/` — SEF-3, T1 testé
- `poc/scenarios/SEF-9-confused-deputy-audit/VERDICT.md` — T12 finding confirmé + correctif validation
- `poc/runtime/src/actor.rs:1243-1260` — `agent_add_cause` sans validation
- `poc/runtime/src/actor.rs:829-895` (rate-limit `0x14`), `cap_denied_resources` — correctif #6 (ADR-0051 §D2)
- `poc/runtime/src/actor.rs:1319-1385` — `agent_self_rollback` séquence log/mutation
- `poc/store/src/lib.rs:97-102` — `ContentStore::put_block` sans fast-path
- `poc/causal-log/src/lib.rs:365-413` — `append` non-fsync, `append_durable` fsync
- Lamport (1978) — "Time, Clocks, and the Ordering of Events in a Distributed System"
- Halpern & Moses (1990) — "Knowledge and Common Knowledge in a Distributed Environment"
- Hardy (1988) — "The Confused Deputy (or why capabilities might have been invented)"
- Harnik, Pinkas, Shulman-Peleg (2010) — "Side Channels in Cloud Services"
- Goguen & Meseguer (1982) — "Security Policies and Security Models"
- Sabelfeld & Myers (2003) — "Language-Based Information-Flow Security"
- Saltzer & Schroeder (1975) — "The Protection of Information in Computer Systems"
- RFC 6962 — Certificate Transparency (Merkle chaining reference)
