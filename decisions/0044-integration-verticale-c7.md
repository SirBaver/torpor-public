# ADR-0044 — Intégration verticale C.7 : N agents, dispatch par badge, serveur séquentiel, non-interférence d'intégrité (I4)

**Date :** 2026-05-29  
**Statut :** Acceptée  
**Amende :** ADR-0038 §Q4 ; précise ADR-0037 §3 ; étend ADR-0043 (portée P6)

## Contexte

C.6 et C.6-crash ont validé `C6_PASS` et `C6-crash_PASS` en régime mono-agent (N=1) : topologie 2-processus, 1 ring SPSC, protocole de commit unitaire figé (ADR-0043 §71).

ADR-0043 §69 (portée bornée P6-C.6) exige une **re-validation P6 dédiée pour C.7** (N agents concurrents). ADR-0043 §73 nomme « l'ordonnancement inter-rings » comme mode de défaillance **nouveau** de C.7, non couvert par C.6.

**Dettes ouvertes :**
- ADR-0038 §Q4 prescrit « agent_id (16 bytes) dans le record » — jamais implémenté (le serveur C.6 n'encode pas agent_id dans le Record, son journal est `Vec<[u8;32]>` sans attribut d'agent).
- ADR-0037 §3 cite un « executor async coopératif » comme partie du runtime seL4 ; sa nécessité reste non-démontrée pour C.7.

**Quatre décisions ouvertes :** (a) où vit l'agent_id, (b) topologie d'exécution multi-agent (N TCB vs executor async), (c) modèle de concurrence côté serveur, (d) re-validation P6 exigée par ADR-0043.

## Décision

**C.7 réalise l'intégration verticale N-agents avec N TCB dans un VSpace partagé, dispatch par badge, serveur séquentiel, et valide P6 (I3-N) + non-interférence d'intégrité (I4). C.7 est découpé en DEUX jalons : C.7-A (intégration nominale) puis C.7-crash (validation P6-N + I4). Le critère de sortie de la phase = C.7-crash PASS.**

## Topologie d'exécution : N TCB, pas executor async (différé)

- **N TCB dans le VSpace runtime partagé** (conforme ADR-0037 Q-seL4-3 : isolation inter-agent = sandbox WASM, pas MMU), chacun avec son ring SPSC et sa commit-cap badgée.

- **Executor async coopératif repoussé.** Il n'existe pas d'événement asynchrone côté commit justifiant un reactor en C.7 — le `seL4_Call` est intrinsèquement synchrone bloquant (ADR-0043 §95). N TCB réutilise le pipeline spawn-task déjà porté en C.6 (ADR-0043 §89), paramétré N fois.

- **Condition déclenchante de l'executor (jalon ultérieur) :** quand (i) N dépasse la capacité CNode/mémoire de N TCB, OU (ii) on porte le cycle eviction/wakeup ADR-0030 (bloqué, ADR-0030 §FutureWork). Tant que N ≤ quelques dizaines, N TCB suffit.

## Badge = agent_id pur, kind dans le label MessageInfo

- **Le badge de la commit-cap encode uniquement l'agent_id** (entier AArch64 64 bits).

- **Le type de requête** (commit vs oracle) est encodé dans le label de MessageInfo.

- **Justification :** séparation propre mécanisme (badge = identité de source, non-forgeable) / sémantique (label = type de requête). Cohérent avec L72 (badge = dispatch oracle déjà en C.6) et L70 (GrantReply obligatoire — `CapRights::all()` pour toutes les commit-caps).

- **L'isolation est garantie par I-cap :** le superviseur mint exactement une commit-cap par agent, badgée avec l'agent_id de cet agent, installée dans le seul CNode de cet agent. Aucun agent ne détient (i) la cap d'un autre agent, (ii) la cap endpoint originale non-badgée, ni (iii) un droit de re-mint (le droit Mint sur l'endpoint est exclusif au superviseur). I-cap est testable : asserter dans C.7-A que le CNode de l'agent A ne contient aucune cap badgée ≠ agent_id(A).

## Serveur séquentiel single-recv

- **Le serveur traite un commit intégralement (ou pas du tout) avant le `recv` suivant.**

- **Conséquence :** I4 tient par (i) survie du serveur (le crash est côté runtime, régime ADR-0038 §41), (ii) traitement série atomique, (iii) le crash d'un agent A à KPᵢ affecte au plus le commit en cours de A, jamais un commit déjà acquitté de B.

- **Coût :** pas de parallélisme de commit côté serveur, débit borné par le throughput de la boucle recv/reply.

## Architecture multi-agent : N rings SPSC distincts, 1 endpoint partagé, N commit-caps badgées

- **1 ring SPSC par agent** (ADR-0038 §32 inchangé), granule 4 KB.

- **1 seul endpoint partagé côté serveur** (simplifie le dispatch par rapport à N endpoints distincts).

- **N caps mintées par le superviseur**, badge = agent_id, rights = `CapRights::all()` (Write + GrantReply, L70 obligatoire).

## Précision ADR-0037 §3 (executor / reactor / scheduler hors-scope C.7)

Le « runtime Rust minimaliste maison » d'ADR-0037 §3 couvre deux rôles distincts :

1. **Reactor IPC seL4** (event loop, multiplexage d'événements IPC) — **différé**.
2. **Scheduler des agents** (portage logique ADR-0030 `IoAdmissionQueue` + `InferencePool`) — **hors-scope C.7**.

- Le **reactor (1)** est différé (pas d'événement asynchrone en C.7 côté commit). 

- Le **scheduler ADR-0030/0031 (2)** est hors-scope C.7 : C.7 ne valide pas l'équité ni les bornes I/O, seulement I3-N et I4. L'ordonnancement entre agents est délégué au scheduler seL4 natif en C.7.

- ADR-0037 §3 citait ADR-0030 comme « réutilisable » — cette affirmation est **partiellement fausse** pour C.7 : ADR-0030 gère l'admission en lecture (réveil, ContentStore), orthogonal au chemin de commit Q3-C de C.7. La réutilisation ADR-0030 est réservée au jalon qui introduit les opérations agent asynchrones non-commit (A1–A4 + agent_infer sur seL4).

## Amendement ADR-0038 §Q4

ADR-0038 §Q4 est amendé comme suit :

> L'agent_id n'est **pas** encodé dans le payload du Record (ring buffer) mais dans le **badge de la commit-cap** mintée par le superviseur (I-cap, ADR-0044 §Badge / §Architecture). Le format Record (kind/hash/payload) figé en C.6 et par l'invariant de stabilité d'ADR-0043 §71 est **inchangé**. L'intégrité de l'agent_id est garantie par le kernel seL4 (badge non-forgeable sous I-cap). Côté serveur, l'agent_id est décodé du badge reçu par `recv(badge)` et stocké dans l'index d'entrées de journal sous la clé `(agent_id, k)`.

## Invariants de C.7-crash (I3-N et I4)

### I3-N — Cohérence état/kill_point par agent (reformulation N-agent de I3-C.6)

Soit A l'agent dont le runtime est suspendu, k l'action en cours d'écriture de A, `seq_A` le numéro de séquence du dernier commit de A dans l'index serveur (interrogé via `Get{agent_id:A, ...}`). I3-N tient ssi :

- **KP1/KP2 ⇒ `seq_A = k-1`** (les blobs et éventuellement le header de l'action k sont des orphelins visibles dans le store content-addressed mais l'action k est absente du journal — licite ADR-0038 §50-52).
- **KP3 ⇒ `seq_A = k-1`** (log_entry pas encore dans le journal serveur, IPC pas retournée).
- **KP4 ⇒ `seq_A = k`** (log_entry dans le journal serveur, IPC retournée).

### I4 — Non-interférence d'intégrité (propriété nouvelle, absente de C.6)

Soit B ≠ A un agent dont j actions ont été committées avant le crash de A, toutes acquittées (seL4_Call retourné). I4 tient ssi après crash de A à tout KPᵢ : `∀ j committé par B avant le crash, Get{agent_id:B, k=j}` retourne l'entrée attendue, inchangée.

**Nature de I4 :** non-interférence d'intégrité (style Biba/Clark-Wilson) portant sur l'état d'index committé observable via `Get`. I4 NE garantit PAS :

- la non-interférence de confidentialité (A n'apprend rien de B — hors-scope) ;
- l'absence de canaux couverts temporels (un crash de A peut retarder B via la sérialisation du serveur single-thread — interférence sur la latence, hors-scope).

**I4 doit être asserté, pas argumenté.** Forme du test : B commit j acquitté (seL4_Call retourné) → A commence commit k et est suspendu à KPᵢ → oracle vérifie `Get(B,j)` inchangé ET `Get(A,k)` conforme à I3-N.

## Portée bornée de la validation P6-C.7

C.7-crash valide P6 **UNIQUEMENT** dans le régime : {N agents, N rings SPSC distincts, serveur séquentiel survivant, **crash d'un seul agent runtime à la fois** aux 4 kill_points Q3-C}.

C.7-crash **NE VALIDE PAS** :

- crashes simultanés de plusieurs agents (1 crash à la fois) ;
- power-loss intégral (serveur mort aussi — ADR-0027 D3, Phase 9+) ;
- non-interférence de confidentialité / canaux couverts temporels (hors-scope) ;
- équité et bornes I/O (scheduler ADR-0030/0031 hors-scope C.7) ;
- ordre inter-agents garanti (le serveur séquentiel donne un ordre de commit global, mais cet ordre n'est pas une propriété garantie par C.7 — il n'y a pas de contrat inter-agents sur l'ordonnancement).

**Clause de compatibilité C.6 :** C.7 étend le protocole unitaire de C.6 par composition (N rings au-dessus de la même primitive seL4_Call), sans modifier l'ordre Q3-C par ring, l'atomicité de l'append du log_entry, ni le format Record. `C6_PASS` et `C6-crash_PASS` restent valides pour le régime N=1.

## Découpage en deux jalons

### C.7-A (intégration nominale N agents) — `C7-A_PASS`

- N=2 agents (2 TCB, 2 rings, 2 commit-caps badgées avec badges distincts), superviseur spawn les 2 runtimes.
- Chacun émet → commit Q3-C → le serveur indexe par `(agent_id, k)` → état lisible et distinct par agent via Get.
- Assertion I-cap : le CNode de l'agent A ne contient aucune cap badgée ≠ agent_id(A).
- NE valide PAS P6 ni I4.

### C.7-crash (validation P6-N + I4) — `C7-crash_PASS`

- Instrumenter KP1-KP4 (même mécanisme `tcb_suspend` self-suspension + signal `suspend_nfn` que C.6-crash, ADR-0043 §32-45).
- Oracle dans le serveur survivant, interrogé via `Get(A, k)` + `Get(B, j)`.
- Asserter I3-N (par agent) ET I4 (non-interférence) dans la portée bornée ci-dessus.
- `C7-crash_PASS` est le critère de sortie de la phase.

### Dépendance stricte

C.7-A est prérequis dur de C.7-crash (même logique ADR-0043 §81 : on ne peut pas instrumenter des kill_points dans une séquence qui ne fonctionne pas).

## Faisabilité seL4 (à vérifier avant de coder C.7-A)

1. **Dimensionnement CNode pour N=2 TCB.** ADR-0043 §97/§135 a mesuré le serveur C.6 à ~121 pages code. Mesurer la taille ELF du runtime C.6 (`readelf -l` sur le binaire aarch64-sel4 release, somme `MemSiz PT_LOAD`) avant de coder : chaque TCB runtime = ~taille_ELF_runtime / 4 KB caps dans le CNode superviseur + son CNode propre + son ring + sa commit-cap. Vérifier que le CNode racine (4096 slots) absorbe le double runtime + serveur + overhead.

2. **Spawn paramétrable.** Vérifier que le pipeline spawn-task de C.6 (ADR-0043 §89) est factorisé ou paramétrable pour N spawns, non hardcodé pour 1 seul fils.

3. **Mint des N commit-caps.** `AbsoluteCPtr::mint(src, rights, badge)` confirmé (ADR-0043 §101). Appliquer `CapRights::all()` (L70 : GrantReply obligatoire) pour chaque commit-cap.

4. **Décodage badge côté serveur.** Valider que le serveur décode `(agent_id, request_kind)` : agent_id = badge, kind = label MessageInfo. Test de décodage avant intégration.

5. **Index serveur par (agent_id, k).** Vérifier que la structure d'index (BTreeMap, Phase 8 RAM) supporte la clé composite `(agent_id, seq)` tout en préservant l'invariant ADR-0038 §3 (l'index est un cache reconstructible depuis le journal — le journal doit stocker l'agent_id décodé du badge pour permettre la reconstruction).

6. **I4 structurel.** Si l'index est une structure globale `BTreeMap<(agent_id,k),entry>`, confirmer que chaque insertion est atomique du point de vue du recv/reply du serveur (pas de mutation observable à mi-chemin entre deux seL4_Call).

## Conséquences

**Positives :**

- Première validation P6 sur seL4 en régime N-agents.
- I4 (non-interférence d'intégrité) établit une propriété de sécurité nouvelle, jamais testée sur Linux.
- Le badge comme vecteur agent_id est plus fort que le payload record (intégrité kernel, gratuit au dispatch).
- Le protocole unitaire de C.6 est préservé (ADR-0043 §71 respecté).

**Négatives :**

- N TCB = N spawns : risque de dimensionnement CNode si N croît (à surveiller, voir Faisabilité).
- Serveur séquentiel : débit borné, pas de parallélisme de commit.
- L'executor async reste à écrire (dette technique, non annulée).
- I4 ne couvre pas les canaux temporels — à noter explicitement pour ne pas surpromettre.

## Options rejetées

| Option | Raison |
|--------|--------|
| **Executor async coopératif (ADR-0037 §3) en C.7** | Pas d'événement asynchrone côté commit ; le `seL4_Call` est synchrone bloquant. N TCB réutilise le pipeline spawn déjà porté sans nouveau code d'infrastructure. |
| **agent_id dans le payload Record** | Modifie le format Record figé en C.6 (ADR-0043 §71 → invalide P6-C.6). Badge strictement supérieur : intégrité kernel, pas de coût de parsing. |
| **Serveur concurrent (commits en parallèle)** | Ouvre un mode de défaillance réel sur l'état partagé du journal, exige une nouvelle preuve d'atomicité. Pas de besoin de débit démontré en C.7. |
| **N rings MPSC partagés** | Brise la garantie Q3-C par ring (plusieurs producteurs peuvent entrelacer blobs/header/log_entry). ADR-0038 §32 exige 1 ring SPSC par agent. |
| **Oracle dans un 3e composant** | Inutile : l'état autoritaire est dans le serveur survivant (même argument qu'ADR-0043 §51). |

## Étapes suivantes

1. Mesurer taille ELF runtime C.6 (`readelf -l poc/sel4-hello/c6-integration/target/aarch64-sel4/release/runtime`) pour dimensionner le CNode superviseur en C.7-A.

2. **C.7-A :** porter le spawn paramétrable (2 TCB runtime), mint 2 commit-caps badgées, serveur badge-dispatch, index `(agent_id, k)`, assertion I-cap. Signal `C7-A_PASS`.

3. **C.7-crash :** instrumenter KP1-KP4 sur un agent (même mécanisme C.6-crash), oracle sur serveur survivant, asserter I3-N + I4. Signal `C7-crash_PASS`.

4. **Executor async (différé) :** déclencheur = besoin de portage A1-A4 + agent_infer asynchrones sur seL4, ou N TCB épuise le CNode.

## Références

- decisions/0043-integration-verticale-c6.md (§71 invariant de stabilité, §69 portée bornée P6, §73 ordonnancement inter-rings, §89/97/135 spawn/CNode/footprint, §32-45 kill_points, §95 seL4_Call synchrone)
- decisions/0038-store-natif-sel4.md (§Q4 amendé, §3 invariant index non-autoritaire, §32 ring SPSC/agent, §41 crash serveur hors-scope, §44-65 Q3-C, §50-52 orphelins)
- decisions/0037-stack-runtime-sel4.md (§3 executor différé, Q-seL4-3 VSpace partagé/isolation WASM)
- decisions/0030-scheduler-unifie-c1-c2.md (IoAdmissionQueue, hors-scope C.7)
- decisions/0031-scheduler-coordinator-reveil-a-la-demande.md (cycle eviction/wakeup, hors-scope C.7)
- decisions/0027-durabilite-log-vs-contentstore.md (D1/D3 régimes crash/power-loss)
- lab/LESSONS.md (L70 GrantReply obligatoire, L72 badge = dispatch oracle C.6, L68 jalon ≠ topologie, L71 mémoire linéaire WASM)
- poc/sel4-hello/c6-integration/ (serveur C.6, absence agent_id dans Record)
- [Goguen & Meseguer 1982] "Security Policies and Security Models", IEEE S&P — non-interférence informationnelle (ce que I4 N'EST PAS)
- [Biba 1977] "Integrity Considerations for Secure Computer Systems" — non-interférence d'intégrité (ce que I4 EST)
- seL4 Reference Manual v15.0.0 §4.2.2 (seL4_Call + badge + GrantReply)
