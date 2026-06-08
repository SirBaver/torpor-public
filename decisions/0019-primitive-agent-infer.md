# ADR-0019 — Primitive `agent_infer`

**Date :** 2026-05-16
**Statut :** Acceptée

---

## Contexte

Le PoC bout-en-bout (`docs/archive/poc_E2E.md` v2) introduit une primitive host
`agent_infer` qui permet à un agent WASM de solliciter une LLM externe
(Ollama, qwen2.5:3b) depuis l'intérieur du sandbox Wasmtime. Cette primitive
est structurellement différente de A1–A4 :

- Elle est *intrinsèquement asynchrone côté hôte* (un appel HTTP, ~2,5 s
  observés L9), alors que A1, A2, A4 sont synchrones.
- Elle s'exerce sur une ressource bornée (`InferencePool` avec sémaphore
  Tokio), contrairement aux autres host functions qui n'ont pas de capacité
  globale.
- Elle ouvre une fenêtre d'attente pendant laquelle un rollback initié par
  le scheduler (ADR-0007, ADR-0010 §0x0B) peut frapper l'agent, ce que
  l'invariant S5 de `actor.rs` (un message à la fois) ne couvrait pas
  jusqu'ici.

La revue architect (`docs/archive/poc_E2E.md` §3.1.3) a identifié sept questions
résiduelles (Q-V2.1 à Q-V2.7) sur la sémantique d'interaction. Cet ADR les
tranche toutes, en plus des questions Q1–Q5 sur l'ABI et le timeout
identifiées dans la v1 du brief.

### Décisions structurantes déjà actées dans le brief (rappel)

- **D-C.** Côté WASM, `agent_infer` est strictement bloquant : l'agent ne
  peut effectuer aucune autre opération pendant l'attente (introspect,
  request_validation, write store, self_rollback).
- Tokio + `Linker::func_wrap_async` : pendant l'attente, le thread Tokio
  est libéré et peut servir d'autres agents.
- Pool d'inférence borné via `tokio::sync::Semaphore` + `CancellationToken`
  par requête en vol (`tokio_util::sync::CancellationToken`).

---

## Décisions

### Q1. Signature ABI WASM

**Décision.** Signature C linéaire, code retour `i32`, payload réponse écrit
dans un buffer fourni par l'agent.

```text
agent_infer(
    prompt_ptr:        *const u8,
    prompt_len:        u32,
    response_buf_ptr:  *mut u8,
    response_buf_cap:  u32,
    response_len_out:  *mut u32,   // longueur réellement écrite (tronquée si > cap)
    timeout_ms:        u32,
) -> i32  // 0=Ok, 1=Timeout, 2=Error, 3=NoSlot, 4=Cancelled
           // Note : code 2 (Error) est aussi retourné si response_buf_ptr ou
           // response_len_out_ptr est hors des bornes de la mémoire WASM
           // (pré-validation Phase 1 — W3, 2026-05-27).
```

**Justification.**

- *Buffer fourni par l'appelant* (vs `*mut *mut u8` alloué côté hôte) :
  évite que la host function gère le memory allocator WASM (qui n'a pas
  d'API standardisée en `wasm32-wasip1`). L'agent SDK alloue son buffer
  (typiquement 8 KiB) et le passe en argument. Aligné avec le style des
  autres host functions A1–A4 (cf. `agent_introspect` qui écrit 74 bytes
  dans un buffer agent).
- *Pas de format structuré* (system prompt séparé, params modèle) au
  niveau de l'ABI : l'agent compose son prompt en string brute. Le format
  Ollama (JSON) est encapsulé dans l'implémentation host. Si un besoin
  apparaît (sampling, temperature, system prompt typé), il fera l'objet
  d'un ADR ultérieur — pas de surface ABI préemptive.
- *Code retour `Cancelled (4)`* nouveau, justifié par Q5.1.
- *Code retour `NoSlot (3)`* : voir Q6 — réservé mais ne sera pas émis en
  Phase 2.

**Troncature de réponse côté agent.** Si la LLM produit plus que
`response_buf_cap` bytes utiles, le buffer est rempli à concurrence de la
capacité et `response_len_out` est positionné à `response_buf_cap`. Un flag
de troncature est tracé dans `InferenceResponse (0x0D)` (cf. Q3). L'agent
peut détecter `response_len_out == response_buf_cap` et inspecter le log
s'il a besoin du flag explicite.

*Détection côté SDK.* Le SDK Rust (`agent-sdk`) expose
`pub fn agent_infer(prompt: &str, timeout_ms: u32) -> Result<InferResponse, InferError>`
avec `InferResponse { text: String, truncated: bool }`. Le flag `truncated`
est calculé par `len_out == response_buf_cap`. Le code retour ABI reste
`Ok (0)` en cas de troncature : la réponse partielle est sémantiquement
utilisable, contrairement à `Timeout/Error/Cancelled/NoSlot` qui signalent
l'absence de réponse exploitable.

### Q2. Sémantique sync/async

**Décision.** Synchrone côté WASM, asynchrone côté hôte (`Linker::func_wrap_async`).

Pendant l'attente, l'agent passe en `LifecycleState::WaitingInference`
(**nouvel état**, ajouté à l'enum dans `actor.rs`). Transition logique :

```text
Active → WaitingInference (entrée agent_infer)
WaitingInference → Active (réponse Ok/Timeout/Error/NoSlot)
WaitingInference → Active (Cancelled : retour immédiat ; le Message::Rollback
                            sera consommé au prochain recv() — cf. Q-V2.1)
```

**Invariant fort (inscrit dans le code par construction).** L'agent est
bloqué dans `agent_infer` au niveau WASM : aucune autre host function ne
peut être appelée tant que l'instruction `call $agent_infer` n'est pas
revenue. C'est garanti par l'exécution séquentielle d'une instance Wasmtime
(pas de threading WASM, pas de réentrance dans `run_loop`). Cet invariant
est la raison pour laquelle `SelfRollback pendant agent_infer` est
impossible (Q5.3).

### Q3. Journalisation (EmitType 0x0C–0x0F)

**Décision.** Quatre nouveaux `EmitType`, ajoutés à `poc/causal-log/src/lib.rs` :

| Code | Nom | Émis | Payload |
|------|-----|------|---------|
| `0x0C` | `InferenceRequest` | À l'entrée de `agent_infer` (après acquisition du slot sémaphore) | `[prompt_hash 32B \| model_id_len u8 \| model_id [u8;N] \| timeout_ms_requested u32 LE \| timeout_ms_effective u32 LE]` |
| `0x0D` | `InferenceResponse` | À réception réussie (avant retour WASM) | `[response_hash 32B \| tokens_estimated u32 LE \| duration_ms u32 LE \| truncated u8]` (`truncated = 1` si réponse > 8 KiB) |
| `0x0E` | `InferenceCancelled` | Future abort-ée (cf. Q5.1) | `[cancel_ts_ms u64 LE \| cause u8]` (`cause = 0x01` Rollback, `0x02` Terminate) |
| `0x0F` | `InferenceFailed` | Erreur (timeout, erreur Ollama, NoSlot) | `[error_code u8 \| message_len u8 \| message [u8;N]]` (message tronqué à 255 bytes) |

**`prompt_hash` et `response_hash` sont SHA-256** sur les bytes UTF-8. Le
texte complet n'est pas inscrit pour deux raisons : (a) borne dure 8 KiB
inline (cf. Q4 §troncature), (b) évite que le log causal devienne une
réplique de toutes les conversations LLM — la *trace* est ce qui compte
pour P3, pas le contenu.

Le texte de la réponse est rendu à l'agent (via `response_buf_ptr`) mais
n'est pas stocké dans le log causal. Si l'agent veut le persister, il doit
l'écrire explicitement dans le `ContentStore` via une action — soumise aux
capabilities.

**Le `prompt_hash` permet la rejouabilité partielle.** Un humain qui
inspecte le log via `os-poc-reconstruct` voit que l'agent a soumis un
prompt de hash X et reçu une réponse de hash Y. Pour debug, le fichier
`reference_responses.jsonl` (§5.3 brief v2) capture les `(prompt, response)`
réels d'un run de référence — pas inscrit dans le log.

### Q4. Erreurs et timeout (double timeout)

**Décision.** Double timeout en couches.

- **`timeout_ms_requested`** : valeur passée par l'agent à `agent_infer`.
  Borne supérieure souhaitée par l'agent (typiquement 60 000 ms pour les
  tests qwen2.5:3b).
- **`host_max_inference_duration_ms`** : borne hôte, configurable au
  scheduler. Défaut : `60_000 ms`. Non négociable.

**Algorithme de clamp :**

```text
timeout_ms_effective = min(timeout_ms_requested, host_max_inference_duration_ms)
```

Le clamp est **observable dans le log causal** : `InferenceRequest (0x0C)`
inscrit *les deux valeurs* (`timeout_ms_requested` ET `timeout_ms_effective`).
C'est la décision tranchée pour Q-V2.4 ci-dessous.

**Pas de retry automatique côté hôte.** Cohérent avec ADR-0014 §c.

**Erreur réseau / Ollama indisponible.** Code `Error (2)`, payload
`InferenceFailed (0x0F)` contient `error_code = 0x10 (network)` et un
message texte tronqué à 255 bytes. L'agent décide de retry, terminer, ou
self_rollback.

**Tableau des codes d'erreur** (champ `error_code` dans `0x0F`) :

| Code | Sens |
|------|------|
| `0x01` | Timeout (sortie code 1 côté WASM) |
| `0x10` | Erreur réseau (Ollama indisponible) |
| `0x11` | Erreur HTTP non-2xx |
| `0x12` | Réponse JSON malformée (Ollama) |
| `0x20` | NoSlot — réservé, non émis en Phase 2 (cf. Q6) |

### Q5. Sémantique d'interaction

#### Q5.1 — `SchedulerRollback` reçu pendant `WaitingInference`

**Décision.** *Abort de la Future + libération immédiate du slot + trace
`InferenceCancelled (0x0E)` + retour code `Cancelled (4)` côté WASM.*

**Mécanisme.**

1. `InferencePool` détient un `CancellationToken`
   (`tokio_util::sync::CancellationToken`) par requête en vol, indexé par
   `agent_id`.
2. Quand `Scheduler::rollback` cible un agent en `WaitingInference`, il
   appelle `token.cancel()` *avant* d'envoyer le `Message::Rollback` dans
   l'inbox de l'agent.
3. La Future Tokio dans `agent_infer` est en `tokio::select!` entre la
   réponse Ollama et `token.cancelled()`. Sur cancellation, elle :
   - émet `InferenceCancelled (0x0E)` avec `cause = 0x01 Rollback`,
   - libère le slot sémaphore (via `Drop` du `OwnedSemaphorePermit`),
   - retourne `InferenceResult::Cancelled` à l'host function,
4. L'host function écrit `response_len_out = 0` et retourne le code `4`
   côté WASM.
5. L'agent reprend la main avec `Cancelled (4)`. À ce stade, `ContentStore`
   est intact — le rollback applicatif n'a pas encore été appliqué.
6. **Q-V2.1 (progression) :** voir ci-dessous.

**Séquence canonique inscrite dans le log causal :**

```text
... → InferenceRequest (0x0C) → InferenceCancelled (0x0E) → SchedulerRollback (0x0B) → ...
```

Cette séquence est *l'observable canonique* d'un rollback pendant
inférence. Le scénario S4 doit la produire pour valider l'invariant.

#### Q5.2 — Capability révoquée pendant `WaitingInference` (TOCTOU)

**Décision.** *Pas de revérification de capability à la sortie de
`agent_infer`.*

**Justification.** `agent_infer` ne touche pas au `ContentStore` et n'est
gouverné par aucune capability d'accès au store. Si l'agent choisit
d'écrire la réponse, ce write traverse `check_capability` standard et
échoue (`capability_denied`) si la cap a été révoquée entre-temps. Pas de
TOCTOU parce qu'il n'y a pas de cache de cap (invariant ADR-0005 :
*"Chaque accès au store re-vérifie"*).

**Invariant inscrit dans cet ADR (référence pour les futurs ADR) :**

> Aucune host function ne met en cache de résultat de capability check.
> Chaque accès au store re-vérifie via `check_capability`. La fenêtre
> temporelle entre vérification et usage est nulle au sens du modèle
> (l'accès store *est* la vérification).

**Note Phase 6.** Si on introduit une capability spécifique "permission
d'appeler la LLM" (rate-limiting par agent), elle serait vérifiée *à
l'entrée* de `agent_infer`, et pas re-vérifiée au retour. Hors scope ici.

#### Q5.3 — `SelfRollback` pendant `agent_infer`

**Décision.** *Cas impossible par construction. Pas de gestion explicite.*

**Justification.** Côté WASM, l'agent est bloqué dans `agent_infer` (Q2).
Il ne peut pas appeler `agent_self_rollback` parce qu'il n'a pas la main.
Aucun autre agent ne peut directement rollbacker un autre agent (seul le
scheduler le peut — couvert par Q5.1).

Inscrit pour traçabilité :

> `SelfRollback` est exclusivement initié par l'agent lui-même via la host
> function `agent_self_rollback`. Comme l'agent ne peut pas appeler cette
> host function pendant qu'il est bloqué dans `agent_infer`, le cas
> 'SelfRollback pendant inférence' n'existe pas.

### Q-V2.1 — Invariant de progression (fenêtre cancel-then-rollback)

**Décision.** *Le `Message::Rollback` est envoyé dans l'inbox **par le
scheduler** immédiatement après `token.cancel()`. La progression est
garantie par S5 (séquentialité de `run_loop`).*

**Justification.** L'invariant S5 (`run_loop`, `actor.rs:2`) garantit que
`run_loop` traite les messages un par un en bouclant sur `inbox.recv()`.
Quand `agent_infer` retourne (avec `Cancelled (4)`), `process_one` se
termine, l'instance WASM rend la main au `run_loop` Rust, qui appelle
immédiatement `inbox.recv().await`. Le `Message::Rollback` est en tête de
file (envoyé par le scheduler avant le `token.cancel()` ou
immédiatement après). Il est consommé au prochain tour.

**Risque résiduel : agent qui boucle sans rendre la main.** Si l'agent
WASM, après retour de `agent_infer` avec `Cancelled (4)`, entre dans une
boucle infinie *à l'intérieur de `process_one`*, il ne revient jamais
à `recv()` et le rollback est bloqué.

**Mitigation Phase 2 (cet ADR) :** intégration de
`wasmtime::Config::epoch_interruption(true)` + `Store::set_epoch_deadline(N_TICKS)`
+ thread d'incrément d'epoch (`Engine::increment_epoch` toutes les
`EPOCH_TICK_MS = 100ms`). Un `process_one` qui dépasse
`MAX_PROCESS_ONE_TICKS * EPOCH_TICK_MS` est interrompu par trap. Constantes
par défaut : `EPOCH_TICK_MS = 100`, `MAX_PROCESS_ONE_TICKS = 50` (= 5 s de
wall clock max par `process_one`). Le temps passé dans `agent_infer` rend
la main au runtime Tokio et ne consomme pas d'epochs WASM — la deadline
ne court que pendant l'exécution active du code WASM. D9 est ainsi résolue
en Phase 2 ; le travail Phase 6 est uniquement le réglage fin des constantes
par classe d'agent.

> Implémentation (3 lignes clés) :
> ```rust
> // Initialisation (1 seul Engine par runtime) :
> let mut cfg = wasmtime::Config::new();
> cfg.epoch_interruption(true);
> let engine = wasmtime::Engine::new(&cfg)?;
> let engine_bg = engine.clone();
> std::thread::spawn(move || loop {
>     std::thread::sleep(std::time::Duration::from_millis(100));
>     engine_bg.increment_epoch();
> });
> // Par Store (réarmé au début de chaque Message::Data dans run_loop) :
> store.set_epoch_deadline(50);
> store.epoch_deadline_trap();
> ```
> Un agent trappé est traité comme `process_one failed` dans `run_loop` :
> log `Terminated`, return. `set_epoch_deadline` doit être réarmé après
> chaque `process_one`, sans quoi la deadline s'épuise sur le premier appel
> long et les suivants trappent immédiatement.

**Contrat post-`Cancelled (4)` (option B).** L'agent est autorisé à
effectuer toute action légale après le retour `Cancelled (4)` jusqu'à la
fin de `process_one`. Ces actions sont visibles dans le log causal et
**seront annulées** par le `SchedulerRollback (0x0B)` consommé au prochain
`recv()` — le `rollback_path` calculé se base sur `target_seq` et défait
toutes les actions store dont la `seq` est supérieure. L'agent n'a aucune
obligation de "céder rapidement" : la séquentialité S5 garantit que le
rollback est traité dès que `process_one` retourne. Le risque "agent qui
boucle" est couvert par le watchdog epoch ci-dessus.

**Conséquence sur l'ordre d'envoi côté `Scheduler::rollback` :**

```rust
// 1. Cancel la Future en cours (libère le slot, trace 0x0E).
inference_pool.cancel(agent_id);
// 2. Envoie Message::Rollback dans l'inbox (consommé après le retour de agent_infer).
inbox.send(Message::Rollback { target_seq }).await?;
// 3. Trace SchedulerRollback (0x0B) — émis depuis le scheduler.
log.append(...);
```

L'ordre `cancel → send → log` produit la séquence canonique
`InferenceCancelled (0x0E) → SchedulerRollback (0x0B)` dans le log.

### Q-V2.2 — Atomicité log lors de la cancellation

**Décision.** *Accepter comme dette (référencée D-Q-V2.2). Pas de
`WriteBatch` groupant `0x0E` et `0x0B` en Phase 2.*

**Justification.** Grouper les deux événements en `WriteBatch` impliquerait
un couplage transactionnel entre `InferencePool` (qui émet `0x0E`) et
`Scheduler::rollback` (qui émet `0x0B`). Ces deux composants ont
aujourd'hui des trajectoires de logging séparées (l'un dans la host
function `agent_infer`, l'autre dans `Scheduler::rollback` côté lib.rs) et
les fusionner demanderait :

1. Un canal de communication explicite (`InferencePool` doit notifier
   `Scheduler` avant de retourner, ou inversement).
2. Une transaction RocksDB cross-composant qui violerait la séparation
   actuelle.

Le coût est disproportionné par rapport au risque : un crash entre `0x0E`
et `0x0B` produit un `InferenceCancelled` orphelin que `os-poc-reconstruct`
sait afficher sans confusion (le payload contient `cause = 0x01 Rollback`
— l'humain comprend qu'un rollback était en cours).

**Inscription dans LESSONS et `TODO.md` sous la dette technique :**

> D-Q-V2.2 (P6 — atomicité crash) : la paire `(0x0E, 0x0B)` n'est pas
> atomique. Sur crash entre les deux, `os-poc-reconstruct` voit un
> `InferenceCancelled` orphelin. Cohérent avec le scope "P6 hors scope du
> PoC" du brief §5.3. À reprendre quand P6 entre dans le scope (Phase 6+).

**Alternative considérée — WriteBatch unifié.** Aurait demandé une
restructuration de l'interface entre `InferencePool` et `CausalLog` :
soit passer le `WriteBatch` ouvert entre composants (fuite
d'abstraction), soit retarder le commit de `0x0E` jusqu'à ce que
`Scheduler::rollback` ajoute `0x0B` (couplage temporel non trivial). Le
gain (atomicité crash sur une transition rare) ne justifie pas le coût en
Phase 2.

### Q-V2.3 — Race condition `0x0D` / `0x0E`

**Décision.** *Le `tokio::select!` est gagné par exactement une branche.
Si Ollama répond avant la cancellation, on émet `0x0D` et on retourne
`Ok (0)` — le rollback restera valide (il sera appliqué au prochain
`recv()`). Si la cancellation gagne, on émet `0x0E` et on retourne
`Cancelled (4)`. **Jamais les deux.***

**Justification.** Sémantique `tokio::select! { biased; ... }` : la branche
`cancel.cancelled()` est polled **en premier** à chaque réveil. Si elle est
`Ready`, la branche Ollama est ignorée même si elle l'était aussi. Cela
formalise l'intention "un rollback déjà décidé l'emporte sur une réponse
arrivée dans le même poll tick". L'invariant exclusion mutuelle de `select!`
reste valide ; `biased;` rend déterministe le choix sous race.

Le `biased;` est **obligatoire** dans toutes les implémentations de
`InferenceBackend::infer` (`OllamaBackend`, `SleepyBackend`, et toute future
impl). Forme canonique :

```rust
tokio::select! {
    biased;
    _ = cancel.cancelled() => InferenceResult::Cancelled,
    res = backend_call()   => /* Ok / Timeout / Error selon res */,
}
```

**Conséquence pour les tests d'intégration.** Le test de cancellation
(S4 ou unitaire) doit assert sur **l'une OU l'autre** des deux séquences,
pas sur "exactement `0x0E`" :

```text
Cas A (cancellation gagne) :
  InferenceRequest (0x0C) → InferenceCancelled (0x0E) → SchedulerRollback (0x0B)

Cas B (Ollama gagne) :
  InferenceRequest (0x0C) → InferenceResponse (0x0D) → SchedulerRollback (0x0B)
```

Le test S4 doit forcer le **cas A** via un `SleepyBackend` configurable
(Q-V2.5) qui ne répond jamais avant cancellation. Le test ne doit pas
dépendre de la chance de gagner la race avec Ollama réel.

**Note.** Si le test passe en Cas B accidentellement, c'est un faux
positif : le scénario `SchedulerRollback pendant inférence en vol` n'a pas
été exercé. Le test doit échouer s'il observe `0x0D` quand il attendait
`0x0E`.

### Q-V2.4 — Clamp `timeout_ms` observable dans le log ?

**Décision.** *Oui. `InferenceRequest (0x0C)` contient à la fois
`timeout_ms_requested` (la valeur fournie par l'agent) et
`timeout_ms_effective` (la valeur après clamp).*

**Justification.** Sans cette double trace, le comportement "Timeout (1)"
côté agent est incompréhensible quand l'agent demande 300 s et est clampé
à 60 s — l'agent croit avoir 300 s, voit un Timeout à 60 s, et le log ne
permet pas de diagnostiquer le clamp.

**Coût.** 8 bytes supplémentaires par `InferenceRequest` (deux `u32 LE`).
Négligeable face au header MessagePack (~30 bytes) + `prompt_hash` (32 B) +
`model_id`. Inscrit dans le format Q3.

### Q6. (ex-Q-V2.6) Politique `NoSlot`

**Décision.** *En Phase 2, `Semaphore::acquire().await` est non borné — la
file d'attente est illimitée. **`NoSlot (3)` ne peut jamais être émis**.
Le code retour est réservé dans l'ABI pour ne pas créer une migration ABI
quand on bornera la file en Phase 6.*

**Justification.** Borner la file (ex. `try_acquire` ou file de capacité
N) implique de définir une politique de rejet :

- FIFO strict avec rejet en queue (drop-newest) ?
- Priorité (cf. `spec/07 §C1.3` priorités sémantiques) ?
- Rejet probabiliste ?

Ces politiques sont *exactement* les propriétés fortes de C1 que le brief
§5.3 reporte explicitement à Phase 6 ("L'équité, la priorité sémantique,
l'absence de famine restent Phase 6"). Trancher une politique en Phase 2
préempterait l'arbitrage Phase 6 sans données.

**Invariant Phase 2 :** *toute requête `agent_infer` qui ne timeout pas
finit par obtenir un slot.* Pas de garantie de latence d'attente, pas de
garantie d'équité.

**Test pour S3.** Le scénario S3 (`inference-cap`) lance N=20 workers,
cap=4. Les 16 en attente doivent **tous finir par compléter** (pas de
famine définitive). C'est un assert simple : `tous les workers ont émis
0x0D ou 0x0F dans la fenêtre du test`. Si une famine est observée, c'est
un bug du sémaphore Tokio (improbable) ou un signal pour Phase 6.

**Migration future.** Quand Phase 6 introduit une borne de file, le code
retour `3` deviendra actif sans changement d'ABI. Les agents qui ignorent
ce code (Phase 2) devront être audités. À noter dans
`spec/02c-primitives-agent.md`.

### Q7. (ex-Q-V2.7) Les `Inference*` events avancent-ils `seq` ?

**Décision.** *Non. `InferenceRequest (0x0C)`, `InferenceResponse (0x0D)`,
`InferenceCancelled (0x0E)`, `InferenceFailed (0x0F)` sont des **événements
de log** : ils n'incrémentent pas `AgentState::seq`. Seules les actions
store (`commit_barrier`) avancent `seq`.*

**Justification.** Cohérent avec le code existant (`actor.rs:642`) :
`seq += 1` est exclusivement dans `commit_barrier`. Les `log_lifecycle_event`
(0x05) n'avancent pas `seq` non plus — le même invariant tient pour les
événements de validation (0x08, 0x09) et de session (0x0A).

`seq` représente la position causale dans le `ContentStore` (séquence
d'actions appliquées au store). Une inférence — même réussie — ne modifie
pas le store ; ce qui modifie le store, c'est la décision *suivante* de
l'agent d'écrire (ou non) la réponse via une action.

**Impact sur `agent_introspect()`.**

Après une séquence :

```text
seq=5  commit_barrier   →  seq devient 6
seq=6  agent_infer ok   →  seq inchangé (0x0C, 0x0D émis)
seq=6  commit_barrier   →  seq devient 7
seq=7  agent_infer cancelled  →  seq inchangé (0x0C, 0x0E émis)
```

`agent_introspect` après le `Cancelled (4)` retourne `seq=7`, pas `seq=8`.
L'agent ne peut pas confondre "j'ai été cancelled" avec "j'ai progressé
dans le store". C'est l'invariant attendu par S4.

**Conséquence pour les rollbacks.** Un `SchedulerRollback` cible une `seq`
qui correspond à un `commit_barrier` réel. Un rollback pendant
`agent_infer` ne risque pas de "rater" la cancellation parce que la
`target_seq` du rollback se rapporte au store, pas aux événements
d'inférence.

**Note de cohérence avec `EmitEnvelope.seq`.** Le champ `seq` dans
`EmitEnvelope` (`poc/causal-log/src/lib.rs:78`) reflète la `seq` de l'agent
*au moment de l'émission*. Pour les `Inference*` events, ce sera la `seq`
de l'agent au moment de l'appel à `agent_infer` — la même valeur pour
`0x0C` et `0x0D`/`0x0E`/`0x0F` correspondants. Cela permet à
`os-poc-reconstruct` de regrouper les événements d'une même inférence par
`seq + agent_id`.

---

## Q-V2.5 — Trait `InferenceBackend` pour les tests de cancellation

**Décision.** *Oui, on introduit un trait `InferenceBackend` avec deux
implémentations : `OllamaBackend` (production, qwen2.5:3b) et
`SleepyBackend` (tests, `tokio::time::sleep` interruptible par
`CancellationToken`).*

**Justification.**

- Le test de cancellation (`Q5.1`) doit s'exécuter en ~10 ms, pas en
  ~2,5 s. Un appel Ollama réel rend le test trop lent pour CI.
- Le test doit forcer le **cas A** (cancellation gagne la race) de façon
  déterministe (Q-V2.3). C'est impossible avec Ollama réel — la latence
  varie.
- `SleepyBackend` peut être configuré avec une `sleep_duration`
  paramétrable ; un test avec `sleep = 10s` et `cancel à T+50ms` gagne
  toujours la race en faveur de la cancellation.
- Le trait permet aussi des `MockBackend` futurs (Phase 6) pour tester
  des réponses adversariales, des erreurs réseau, etc.

**Forme du trait (esquisse).**

```rust
#[async_trait]
pub trait InferenceBackend: Send + Sync {
    async fn infer(
        &self,
        prompt: &str,
        timeout_ms: u32,
        cancel: CancellationToken,
    ) -> InferenceResult;
}

pub enum InferenceResult {
    Ok { response: String, duration_ms: u32, tokens: u32 },
    Timeout,
    Error(String),
    Cancelled,  // observé via cancel.cancelled()
}
```

`InferencePool` est paramétré par un `Arc<dyn InferenceBackend>`. La
production injecte `OllamaBackend`, les tests injectent `SleepyBackend`.

**Alternative considérée — test avec Ollama réel uniquement.** Rejetée :
- Tests d'unité dépendraient d'un service externe (Ollama doit tourner).
- Latence ~2,5 s × N tests fait dérailler le CI.
- La race condition Q-V2.3 ne pourrait être forcée déterministe.

**Conséquence sur l'archi.** Le crate `runtime` exporte `InferenceBackend`
comme trait public. `OllamaBackend` est dans `runtime` ou un sous-module
`runtime::backends::ollama`. `SleepyBackend` est dans `#[cfg(test)]` ou un
sous-module `runtime::backends::test`.

---

## Alternatives considérées

### Q5.1 — Détachement de la Future en cours

*Laisser la Future continuer après cancellation, ignorer son résultat.*

Rejetée. Crée des slots zombies : le sémaphore reste occupé tant que la
Future détachée n'a pas terminé. En cas de charge soutenue, on peut avoir
N agents en `WaitingInference` alors que la cap=4 est saturée par des
Futures zombies. Fausse l'observabilité du pool. Viole l'invariant de
borne dure visible (P1 / S3).

### Q5.1 — Rollback différé (attendre la fin de l'inférence)

*Attendre la complétion de la Future avant d'appliquer le rollback.*

Rejetée. Une inférence peut durer jusqu'à `host_max_inference_duration_ms`
(60 s par défaut). Retarder un rollback de 60 s viole P2 (rollback borné
en temps). De plus, la réponse LLM reçue post-rollback est sémantiquement
invalide (l'agent a été rollbacké à un état antérieur ; la réponse se
réfère à un contexte qui n'existe plus). Inscrire cette réponse dans le
log produirait des entrées causalement orphelines.

### Q-V2.2 — WriteBatch unifié `(0x0E, 0x0B)`

Voir §Q-V2.2 ci-dessus. Rejetée : coût d'intégration disproportionné par
rapport au gain (atomicité crash sur transition rare). À reconsidérer
quand P6 entre dans le scope.

### Q-V2.5 — Tests avec Ollama réel uniquement

Voir §Q-V2.5 ci-dessus. Rejetée : latence, dépendance externe, race non
forcable déterministe.

### Q3 — Inscrire le texte complet du prompt/réponse dans le log

Rejetée. Avec une borne 8 KiB par entrée (brief §5.3), un prompt + réponse
de plusieurs KiB ferait sortir la majorité des `Inference*` events en
BlobDB (ADR-0017), avec un coût p99 sur les lookups T5. De plus, transformer
le log causal en réplique conversationnelle confond deux objets : *trace
causale* (P3, doit rester compact) et *capture de conversation* (debug,
doit aller dans `reference_responses.jsonl`).

### Q1 — Buffer alloué côté hôte (style `*mut *mut u8`)

Rejetée. Demande à la host function d'invoquer l'allocator WASM (pas d'API
standard en `wasm32-wasip1`). Demande à l'agent de libérer le buffer
(ownership cross-FFI fragile). Style cohérent avec A1–A4 : buffer fourni
par l'appelant.

---

## Conséquences

### Nouveaux types

- **`EmitType`** étendus : `InferenceRequest (0x0C)`, `InferenceResponse
  (0x0D)`, `InferenceCancelled (0x0E)`, `InferenceFailed (0x0F)`. Ajouter à
  `poc/causal-log/src/lib.rs` enum + `TryFrom<u8>`.
- **`LifecycleState::WaitingInference`** : nouvelle variante de l'enum
  dans `poc/runtime/src/actor.rs`. Transitions documentées en Q2.
- **Code retour `Cancelled (4)`** : ajouté à l'ABI WASM. Le code `NoSlot
  (3)` est réservé mais non émis en Phase 2 (Q6).
- **Trait `InferenceBackend`** + impls `OllamaBackend`, `SleepyBackend`
  (Q-V2.5).

### Modifications de modules

- `poc/runtime/` : ajout `agent_infer` host function, `InferencePool`,
  trait `InferenceBackend`, état `WaitingInference`. Nouvelle dépendance
  `tokio-util` pour `CancellationToken`.
- `poc/causal-log/` : ajout 4 `EmitType` + variants `TryFrom`.
- `poc/reconstruct/` : ajout résumés payload pour `0x0C–0x0F` (compléter la
  table ADR-0018).
- `spec/02c-primitives-agent.md` : ajout section A5 décrivant `agent_infer`
  + table des codes retour + l'invariant fort de blocage.

### Invariants à préserver

- **`seq` n'avance pas sur `Inference*` events** (Q7).
- **Aucun cache de capability** (Q5.2).
- **Exclusion mutuelle `tokio::select!`** : exactement une branche gagne,
  jamais les deux (Q-V2.3).
- **Ordre `cancel → send Rollback → log SchedulerRollback`** dans
  `Scheduler::rollback` quand l'agent cible est en `WaitingInference`
  (Q-V2.1).

### Impact ADR existants

- **ADR-0005 / ADR-0007** (capabilities) : non impacté. Q5.2 montre que
  l'invariant "pas de cache de cap" couvre déjà le cas `agent_infer`.
- **ADR-0010** (contrat `emit`) : étendu par les 4 nouveaux EmitType. Pas
  d'amendement formel — ADR-0010 prévoit explicitement que les types
  `0x0B–0xFF` sont réservés et qu'on peut en ajouter.
- **ADR-0014** (politique supervision) : non impacté. Le timeout
  `agent_infer` est distinct du timeout de validation A3 (ADR-0014
  §D14.b).
- **ADR-0017** (BlobDB) : non impacté à court terme — les payloads
  `Inference*` events sont ≤ 100 bytes (hashes + petits champs). Aucun
  n'atteindra le seuil 4 KiB BlobDB.
- **ADR-0018** (reconstruct) : à étendre — ajouter les résumés `0x0C–0x0F`
  dans la table §Résumés de payload par type.

### Dettes techniques inscrites

- **D9 — résolue Phase 2** : watchdog d'instruction WASM intégré via
  `epoch_interruption` (Q-V2.1). Travail Phase 6 résiduel : calibration fine
  des constantes `EPOCH_TICK_MS` / `MAX_PROCESS_ONE_TICKS` par profil d'agent.
- **D-Q-V2.2** (P6) : atomicité crash `(0x0E, 0x0B)` (Q-V2.2).
- **D-Q-V2.6** (Phase 6) : politique de rejet `NoSlot` quand la file
  d'inférence sera bornée.

À ajouter à `TODO.md` sous la section dettes techniques.

---

## Conditions d'acceptation

- Q1–Q7 toutes tranchées avec décision et justification (✓).
- Q-V2.1 à Q-V2.7 toutes tranchées (✓).
- Sections "Alternatives considérées" remplies pour Q5.1 (deux options),
  Q-V2.2 (WriteBatch), Q-V2.5 (tests sans backend) (✓).
- Conséquences sur log causal (4 nouveaux EmitType) listées (✓).
- Conséquences sur `LifecycleState` (ajout `WaitingInference`) listées (✓).
- Conséquences sur ADR-0005, ADR-0007 (révocation caps) : aucune (Q5.2 le
  montre) (✓).
- Dettes Phase 6 explicitement nommées (D9, D-Q-V2.2, D-Q-V2.6) (✓).

---

## Références

- `docs/archive/poc_E2E.md` v2/v3 — brief de chantier, §3.1 (ADR-0019), §5.3
  (dettes techniques)
- ADR-0005 — Design capabilities (invariant "pas de cache de cap")
- ADR-0007 — Invalidation caps lors d'un rollback (Q5.2)
- ADR-0010 — Contrat `emit` (EmitType, format MessagePack)
- ADR-0011 — Options RocksDB (CF `agent_ts`)
- ADR-0014 — Politique supervision (timeout, pas de retry — cohérent Q4)
- ADR-0017 — BlobDB sur CF `default` (impact négligeable, Inference*
  events restent inline)
- ADR-0018 — `os-poc-reconstruct` (à étendre pour 0x0C–0x0F)
- `poc/runtime/src/actor.rs` — `run_loop`, `commit_barrier`, host
  functions A1–A4
- `poc/causal-log/src/lib.rs` — `EmitType`, `EmitEnvelope`
- Wasmtime `Linker::func_wrap_async` :
  https://docs.rs/wasmtime/latest/wasmtime/struct.Linker.html#method.func_wrap_async
- `tokio_util::sync::CancellationToken` :
  https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html

---

*Format inspiré de MADR (Markdown Architecture Decision Records) — [Nygard 2011]*
