# FINDING-2 — Slot zombie après cancel pendant `WaitingInference`

**Vecteur :** A-2  
**Propriétés attaquées :** P2 × C1  
**Use case de référence :** UC-10 / S16  
**Régime :** R1 (effets) + R2 (ressources)

---

## Hypothèse d'attaque

Un agent est en état `WaitingInference`. Un rollback (ou cancel) est déclenché pendant l'attente. Hypothèse : le slot d'inférence (sémaphore) reste acquis après la transition d'état → slot zombie, `available_permits()` décrémenté définitivement → C1 (borne dure) violée à terme.

## Oracle

`poc/scenarios/S16-infer-cancel-toctou/VERDICT.md` — harnais Rust déterministe.

Invariant binaire : `available_permits_before == available_permits_after` une fois l'agent revenu en `Active` ou `Terminated`. Si les permits ne sont pas restaurés, l'oracle émet FAIL.

## Résultat

**PASS**

Sur rollback ciblant un agent en `WaitingInference` : `CancellationToken` déclenché avant `Message::Rollback`, slot libéré immédiatement via drop `OwnedSemaphorePermit`. Aucun zombie observé. `available_permits == POOL_CAP` confirmé après rollback. Séquence log `0x0C < 0x11 < 0x0E < 0x0B < 0x12` respectée.

Oracle exécuté : `tests::s16_infer_cancel_toctou` (1 run, POOL_CAP=4, SleepyBackend 60s). Voir `poc/scenarios/S16-infer-cancel-toctou/VERDICT.md`.

## Classification

**Propriété tenue** — aucun correctif requis.

## Notes

- La libération du slot est couplée à la transition d'état dans le scheduler, pas dans la logique applicative de l'agent. Cela ferme la fenêtre TOCTOU.
- Le cas cancel + re-soumission immédiate (race entre libération et nouvelle acquisition) est couvert par le sémaphore Tokio — pas d'état intermédiaire observable.
