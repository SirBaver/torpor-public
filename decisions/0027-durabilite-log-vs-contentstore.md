# ADR-0027 — Régime de durabilité du log causal vs ContentStore (P6 sous crash brutal)

**Date :** 2026-05-18
**Statut :** Acceptée
**Amende :** ADR-0024 §D1 (clarification du régime de durabilité de `0x11/0x12`),
ADR-0010 §Contrat `emit()` (clarification du contrat de durabilité d'`append`),
`spec/02-properties.md §P6` (précision de la portée du modèle de menace)

---

## Contexte

L'ajout de `CausalLog::append_durable()` (commit 2026-05-18) — qui force
`WriteOptions::set_sync(true)` sur le WAL RocksDB pour mesurer P3b — pose la
question du **régime de durabilité réellement requis** sur le chemin chaud du
runtime ADR-0024 :

- 5 sites « crash-critical » émettent des `LogEntry` sur la trajectoire d'un
  rollback scheduler :
  - `0x11 CompensationOpen` (`scheduler.rs::emit_compensation_open`, ADR-0024 D1)
  - `0x0E InferenceCancelled` (`actor.rs` host fn `agent_infer` branche
    `Err(Cancelled)`, ADR-0019)
  - `0x0B SchedulerRollback` (`actor.rs` `run_loop` bras `Message::Rollback`,
    ADR-0007 + D5)
  - `0x12 CompensationClose` (`scheduler.rs::emit_compensation_close`, ADR-0024 D1)
  - `0x13 AgentCrash` (`actor.rs::log_agent_crash`, ADR-0015 D15.2)

Tous appellent `CausalLog::append()` (non-durable, pas de fsync forcé). Question :
faut-il les basculer sur `append_durable()` pour respecter P6 ?

Cet ADR tranche en clarifiant **trois invariants distincts qui étaient implicites
dans les ADRs précédents** :

1. P6 est défini sur **l'état du système**, mesuré par hash, et cet état est
   logé dans le ContentStore (Merkle DAG) — pas dans le CausalLog.
2. Le modèle de menace effectif des SEFs Phase 6 est `SIGKILL` /
   `std::process::exit` / panic Rust — **pas** la coupure secteur. RocksDB
   garantit l'absence de perte sur ces crashes via le WAL OS-buffered (le page
   cache du noyau survit à la mort d'un processus utilisateur).
3. L'atomicité D-Q-V2.2 d'ADR-0024 est **détective**, pas constituante de P6 :
   son rôle est d'aider l'opérateur via `os-poc-reconstruct` ; sa perte ne
   viole pas P6.

---

## Décision

### D1. Aucun site ADR-0024 / ADR-0015 ne force `append_durable`

Les cinq sites listés ci-dessus restent sur `append()` (WAL non-fsynced). Pas
de bascule.

### D2. `append_durable` reste réservée à la mesure P3b (T5-bis) et à des sites futurs explicitement justifiés

L'API `append_durable()` est conservée parce qu'elle est requise par P3b
(spec/02 §P3b) — la mesure end-to-end `emit → fsync → get` n'a de sens que si
fsync est forcé. Mais elle n'est appelée par **aucun site runtime** dans
l'état actuel. Tout site qui voudrait l'appeler doit produire une
justification écrite (ADR ou amendement) répondant à : « quel invariant est
brisé sous power-loss si l'écriture n'est pas fsynced ? ».

### D3. P6 est ré-énoncée avec son modèle de menace effectif

`spec/02-properties.md §P6` est amendée pour distinguer deux régimes :

- **Régime SIGKILL/panic (couvert)** — crash brutal du processus utilisateur,
  page cache OS préservé. Toutes les écritures RocksDB depuis l'ouverture
  survivent (rejouées au démarrage par recovery WAL). C'est le régime de
  SEF-4. **P6 tient par construction sans fsync.**

- **Régime power-loss (non couvert en Phase 6)** — kernel panic, coupure
  secteur, hardware fault. Les écritures depuis le dernier fsync sont perdues.
  La couverture de ce régime nécessite : (a) `set_sync(true)` sur les commit
  barriers (`put_block` + `put_snapshot` + `append` du Lifecycle ou
  `SchedulerRollback`), (b) garanties matérielles (NVMe avec PLP, fsync
  honnête) ; mesurée par P3b. **Phase 7+ ; pas un objectif Phase 6.**

### D4. Si Phase 7+ promeut la couverture power-loss, l'unité fsync est la commit barrier — pas la compensation

La règle énoncée pour le futur : si l'on veut garantir P6 sous power-loss, on
fsync **autour de la commit barrier d'action** (au moment où l'agent fait
avancer `seq` et écrit un nouveau `SnapshotHeader` dans le ContentStore). On
ne fsync **pas** autour de la transaction de compensation 0x11/0x12, parce
que la compensation n'écrit pas le ContentStore et que la perte de la
compensation sous power-loss est équivalente à « la transaction n'a jamais
commencé » — exactement l'état avant transaction de P6.

---

## Justification — pourquoi forcer fsync sur 0x11/0x0E/0x0B/0x12/0x13 ne résoudrait rien

**Cas A — perte de 0x11 (CompensationOpen).**
*Effet observable :* aucun (rien dans le log persisté).
*Cohérence P6 :* OK. ContentStore inchangé. Au recovery, l'agent reprend à
son état pré-rollback, ce qui est l'état « avant transaction » de P6.

**Cas B — 0x11 persisté, 0x0E perdu.**
*Effet observable :* `os-poc-reconstruct` voit 0x11 sans suite. Affiche
`[INCOMPLETE COMPENSATION]` (ADR-0024 D3, déjà implémenté
`poc/reconstruct/src/main.rs::check_compensation_journal`).
*Cohérence P6 :* OK. ContentStore inchangé.

**Cas C — 0x11 + 0x0E persistés, 0x0B perdu.**
*Effet observable :* idem cas B (0x11 sans 0x12, INCOMPLETE COMPENSATION).
*Cohérence P6 :* OK. **C'est l'observation cruciale :** le bras
`Message::Rollback` d'`actor.rs:1703-1813` n'écrit **rien dans le
ContentStore**. `rollback_path` est un *lookup* de chaîne de snapshots
existante ; `revoke_owned_after` mute `CapabilityStore` en mémoire ; le
seul effet persistant est l'écriture de 0x0B dans le log. Perdre 0x0B
équivaut donc à « le rollback n'a jamais été appliqué » — l'agent au
redémarrage repart de `last_snapshot` antérieur. État avant transaction.

**Cas D — 0x11 + 0x0E + 0x0B persistés, 0x12 perdu.**
*Effet observable :* INCOMPLETE COMPENSATION (auto-close + warning).
*Cohérence P6 :* OK. Le rollback applicatif est tracé (0x0B avec
`hash_after = target_snap`) ; au redémarrage le scheduler n'existe plus, mais
le ContentStore est cohérent, et le log porte une trace fidèle de la
transition (avec la mention « close manquant » qui est diagnostique, pas
correctrice).

**Le seul cas pathologique** serait un mode de durabilité **mixte** où 0x12
fsync mais 0x0B non, ou 0x11 fsync mais 0x0E non. Dans ces cas, le log peut
contenir une paire (0x11, 0x12) sans le 0x0B intermédiaire, faisant croire à
une compensation aboutie alors que l'application du rollback côté agent n'a
laissé aucune trace. C'est précisément ce que **un régime uniforme `append()`
évite** : soit toute la séquence survit (page cache OS), soit aucun élément
ne survit (power loss intégral) — toujours diagnosticable, jamais ambigu.

**Cas E — 0x13 AgentCrash perdu.**
*Effet observable :* l'agent est mort (run_loop terminé) mais aucun
`AgentCrash` n'apparaît dans le log. Au redémarrage, l'agent est absent de la
liste des acteurs vivants ; sa dernière trace dans le log est son
`Lifecycle::Active` ou autre événement antérieur. Pas de cause documentée.
*Cohérence P6 :* OK. Le ContentStore de l'agent reflète son dernier état
committé. **Coût accepté :** la perte de traçabilité de cause sur power-loss
est inscrite comme limite documentée d'ADR-0015 (à amender).

---

## Coût de l'alternative rejetée (forcer `append_durable` partout)

- **Latence** : 0,5–15 ms par appel selon hardware (cf. P3b). `Scheduler::rollback`
  ferait au minimum 2 × fsync (0x11, 0x12) ; un agent qui boucle `commit_barrier
  + emit` paierait fsync à chaque action. Coût massif vs régime actuel
  ~20 µs/append (mesuré T5).
- **Bénéfice nul sous SIGKILL/panic** (page cache OS préservé, RocksDB rejoue).
- **Bénéfice partiel sous power-loss** : sans fsync coordonné sur le
  ContentStore (`put_block`/`put_snapshot`), forcer fsync uniquement sur le
  log produit un log « en avance » sur le store — log dit que l'agent a
  écrit X, mais X absent du ContentStore au redémarrage. **Pire que le statu
  quo** parce qu'introduit une incohérence asymétrique observable.

La conclusion : la couverture power-loss est une **propriété du chemin
ContentStore + commit barrier**, pas du chemin de compensation. Toute
introduction de `append_durable` sur le log doit être *précédée* d'une
introduction symétrique de fsync sur ContentStore. Phase 7+.

---

## Alternatives considérées

| Alternative | Avantages | Inconvénients | Raison du rejet |
|-------------|-----------|---------------|-----------------|
| **A1. Forcer `append_durable` sur 0x11/0x12** (compensation seule) | Marqueur transactionnel garanti durable | Asymétrie 0x11/0x12 fsynced vs 0x0E/0x0B non fsynced → faux positifs au recovery sous power-loss ; coût ×100–×1000 latence ; aucun bénéfice sous SIGKILL | Rejetée. Crée un mode pathologique de log incohérent. |
| **A2. Forcer `append_durable` sur tous les 5 sites** | Atomicité log uniforme sous power-loss | Coût massif sans fsync corrélatif sur ContentStore ; log « en avance » sur store ; latence rollback ~30 ms ; aucun bénéfice sous SIGKILL/panic | Rejetée. Bénéfice illusoire sans fsync ContentStore corrélé. |
| **A3. Forcer `append_durable` sur tous les sites + fsync ContentStore (`put_block`/`put_snapshot`)** | Couvre power-loss intégral | Refonte du chemin d'écriture ContentStore ; coût action-par-action ×100 ; non mesuré ; dépend de T5-bis non encore exécuté | **Reportée Phase 7+.** Critère de déclenchement : T5-bis montre p99 fsync ≤ 5 ms sur hardware qualifié + SEF-4 power-loss promu. |
| **A4. `append_durable` réservée P3b, statu quo runtime** *(retenue)* | Pas de régression latence ; P6 tient par construction sous SIGKILL ; compensation détective inchangée | Power-loss non couvert (assumé hors scope Phase 6, cohérent SEF-4) | **Retenue.** |
| **A5. Supprimer `append_durable`** | Surface API minimale | Casse mesure P3b (T5-bis bloquée) ; P3b est dans la spec | Rejetée. P3b est une borne de performance déclarée. |

---

## Conséquences

**Positives :**

- Performance Phase 6 préservée : pas de fsync sur le chemin chaud rollback.
- Cohérence du log garantie : régime de durabilité uniforme, pas de mode
  mixte pathologique.
- Spec §P6 reformulée avec son modèle de menace effectif (SIGKILL), plus
  honnête que l'énoncé « perte de courant » qui n'était pas couvert et que
  l'ajout d'`append_durable` n'aurait pas couvert seul.
- ADR-0024 D1 est confirmée comme stratégie détective et le reste — pas
  d'illusion de garantie cryptographique.

**Négatives / coûts acceptés :**

- Power-loss reste non couvert en Phase 6. C'est un trou documenté dans P6.
- Si un opérateur lit un log post-power-loss, certains événements peuvent
  être absents. La règle est : **ContentStore est authoritative pour l'état ;
  le log est best-effort sous power-loss**.

**Neutres / à surveiller :**

- Quand T5-bis aura mesuré p99 fsync sur hardware qualifié, la décision
  Phase 7+ sur la couverture power-loss devient quantifiable. Si p99 fsync
  ≤ 5 ms, l'extension à `append_durable` sur les commit barriers est
  envisageable sans tuer P1b ; si p99 fsync ≥ 20 ms, il faudra arbitrer
  P1b (débit) vs P6 (power-loss).
- ADR-0015 doit recevoir une note : « AgentCrash (0x13) survit sous SIGKILL
  via WAL OS-buffered ; sous power-loss, perte possible et acceptée ». Pas
  un amendement formel, juste une clarification.

---

## Observation post-décision — découverte SEF-4 (2026-05-18)

Le scénario S6 (`poc/scenarios/S6-crash-atomicity/`, 40 runs sur 4 kill_points × 2
actions × K=5) valide empiriquement P6 sous le régime SIGKILL — mais avec une
nuance importante :

**ADR-0027 D3 affirme « toutes les écritures RocksDB depuis l'ouverture
survivent (rejouées au démarrage par recovery WAL) ». Cette formulation est trop
forte.** L'observation SEF-4 montre que sous `process::exit(1)` sans destructeur
RocksDB exécuté :

- Le préfixe d'actions effectivement récupéré post-recovery est **strictement
  préfixe** du préfixe d'actions exécutées. Plusieurs actions terminées avant
  le kill peuvent être absentes du log post-recovery.
- Distribution observée sur 40 runs (kill armés à actions 3 et 4) :
  - 25 runs : `observed = pre[k]` (l'action `k` n'est pas committed côté log)
  - 8 runs : `observed = pre[k-1]` ou `pre[k-2]` (l'action `k-1` voire `k-2` n'est pas committed)
  - 7 runs : `observed = post-action-i` avec `i ∈ {k-1, k}` (l'action de kill,
    ou l'action précédente, est committed)

**P6 (atomicité par action) tient strictement** : aucun état observé n'est partiel
(jamais de block sans header pointant dessus, jamais de header sans entrée log dans
la chaîne reconstructible). C'est ce que SEF-4 valide réellement.

**Cause racine de la perte d'actions multiples** : RocksDB en `WriteOptions::default()`
(WAL non-synced) bufferise applicativement les écritures au-delà du page cache OS.
`db.write(batch)` retourne `Ok` après avoir poussé dans la memtable et un buffer
WAL interne, mais le `write(2)` syscall vers le file descriptor du WAL peut être
différé. Sous `process::exit(1)`, ce buffer est perdu — y compris pour des actions
qui ont retourné `Ok` plusieurs cycles avant le kill.

**Conséquence pour P6** : la propriété telle qu'énoncée (« observable entièrement
ou pas du tout ») est satisfaite. Mais la propriété qu'un opérateur attend
intuitivement (« si `append()` a retourné OK, l'écriture survit à un crash
processus ») **n'est pas vraie** sans bytes_per_sync ou manual_wal_flush.

**Décision (clarification ADR-0027 D3)** :

- P6 est définie sur l'état observable post-recovery. SEF-4 la valide.
- Le contrat « append() est durable sous SIGKILL » n'a jamais été énoncé
  formellement. ADR-0027 le sous-entendait par « toutes les écritures
  survivent » — cette phrase est ré-énoncée comme « toutes les écritures
  qui ont été flushées au moins jusqu'au page cache OS au moment du kill
  survivent ; RocksDB peut buffer en deçà, et la frontière exacte dépend
  de `WriteOptions::default()` ».
- Phase 7+ : si l'on veut une garantie plus forte (« append OK ⇒ écriture
  visible post-recovery sous SIGKILL »), activer `manual_wal_flush(false)`
  + `bytes_per_sync(1)` sur le log, **ou** introduire `append_buffered_flush()`
  qui force le `write(2)` syscall sans `fsync`. Coût attendu : 10–100 µs
  par append (vs ~5 µs actuels — facteur 2–20×, à mesurer).

**Cette note ne modifie pas la décision D1–D4 d'ADR-0027.** Elle clarifie
le contrat effectif observé.

---

## Sites concernés — annotations à apporter

Les cinq sites doivent porter un commentaire bref pointant cet ADR. Format
proposé :

```rust
// ADR-0027 : append() non-durable suffit. P6 est garantie via recovery WAL
// RocksDB sous SIGKILL/panic ; power-loss non couvert Phase 6 (cf. §D3).
```

À ajouter dans :
- `poc/runtime/src/scheduler.rs::emit_compensation_open` (0x11)
- `poc/runtime/src/scheduler.rs::emit_compensation_close` (0x12)
- `poc/runtime/src/actor.rs` host fn `agent_infer` bras `Err(Cancelled)` (0x0E)
- `poc/runtime/src/actor.rs` bras `Message::Rollback` (0x0B)
- `poc/runtime/src/actor.rs::log_agent_crash` (0x13)

---

## Références

- ADR-0024 — Atomicité crash via journal de compensation (cet ADR confirme
  la nature « détective » de la stratégie J)
- ADR-0019 — Primitive `agent_infer` (sites 0x0C–0x0F)
- ADR-0015 — Propagation erreur cross-agent (AgentCrash 0x13)
- ADR-0010 — Contrat `emit()` (CausalLog::append)
- ADR-0011 — Options RocksDB Layer 0 (WAL, options de durabilité)
- ADR-0026 — Régime de cache de référence P3a (parent thématique : régime de
  mesure ; cet ADR-0027 traite du régime de durabilité)
- `spec/02-properties.md §P6` — Atomicité crash (à amender pour clarifier le
  modèle de menace)
- `spec/02-properties.md §P3b` — Borne ≤ 20 ms emit→fsync→get (justifie
  l'existence de `append_durable`)
- `poc/causal-log/src/lib.rs::append` — implémentation non-durable
- `poc/causal-log/src/lib.rs::append_durable` — implémentation fsync (P3b)
- [Gray & Reuter 1992] *Transaction Processing* — taxonomie « durable »
  (force-at-commit) vs « no-force » (group commit + recovery) ; cet ADR est
  un mode « no-force » côté log, couplé à un ContentStore qui est lui-même
  « no-force » sous le régime de menace SIGKILL.
- TiKV `fail-rs`, FoundationDB Buggify — pattern failpoint (déjà cité
  ADR-0024) ; ne touche pas à la durabilité, mais à l'injection de pannes.

---

*Format inspiré de MADR — [Nygard 2011] "Documenting Architecture Decisions"*
