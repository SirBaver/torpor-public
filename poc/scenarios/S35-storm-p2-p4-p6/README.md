# S35 — Tempête P2×P4×P6 (UC-23 / ADR-0001 / ordre d'arbitrage)

**Régime :** R1 (P2 × P4 × P6 — propriétés d'effet combinées)
**Substrat :** Linux. Non transférable à seL4 (D7).

---

## Ce qui est testé

**ADR-0001 (ordre d'arbitrage P4 ≻ P2 ≻ P6)** : vérifier que les trois propriétés tiennent
simultanément sous « tempête » — événements concurrents stressant les trois axes à la fois.
UC-23 est le test d'intégrité de l'ordre d'arbitrage. Contrairement aux tests unitaires
(S16 = WaitingInference seul, S17 = P2×P4 seul), S35 superpose :

| Événement | Propriété | Observé |
|-----------|-----------|---------|
| Agent A en `WaitingInference` pendant le rollback | P2 ≻ P6 | InferenceCancelled (0x0E) + SchedulerRollback (0x0B) |
| Rollback révoque C_A2 (post-S0) + cascade C_B → B | P4 ≻ P2 | check(B, C_B) == false |
| Journal de compensation (0x11/0x12) complet | P6 | CompensationOpen + CompensationClose |
| Agent B atteint sa frontière de session avant la tempête | P3/P1b | SessionBoundary (0x0A) |

---

## Scénario

```
t=0ms     : C_root octroyée à A (pré-S0, survivra au rollback)
t=0ms     : Acteur A (INFER_AGENT_WAT, SleepyBackend 60s), acteur B (AGENT_WAT, max_actions=2)

t=1ms     : A → msg[0x00] → commit_barrier → snapshot S0
t=80ms    : S0 stabilisé

t=80ms    : C_A2 octroyée à A (post-S0 → sera révoquée)
t=80ms    : C_B déléguée de C_A2 vers B (sera révoquée en cascade)

t=80ms    : B.process_one([0x00]) → action 1/2
t=80ms    : B.process_one([0x00]) → action 2/2 → SessionBoundary (max_actions=2)

t=80ms    : A → msg[0x07] → agent_infer → WaitingInference (SleepyBackend bloque 60s)
t=160ms   : pool.is_active(A) == true ✓

t=160ms   : scheduler.rollback(A, target_seq=0)
             → CompensationOpen (0x11)
             → cancel(A) → InferenceCancelled (0x0E)
             → Message::Rollback → SchedulerRollback (0x0B) + revoke_owned_after(A, S0_ts_ms)
               → revoke(C_A2) → cascade → revoke(C_B)
               → caps_invalidated = 2
             → CompensationClose (0x12)

t=560ms   : assertions
```

---

## Oracles

### P4 (priorité maximale)
```
C_root (pré-S0) : get(c_root) == Some(_)   ← non révoquée
C_A2  (post-S0) : get(c_a2)  == None       ← révoquée par rollback
C_B   (cascade) : get(c_b)   == None       ← révoquée en cascade
check(B, C_B, "/data/sub") == false         ← isolation tient sous tempête
```

### P2
```
InferenceCancelled (0x0E) dans le log de A  ← A était en WaitingInference
SchedulerRollback  (0x0B) dans le log de A
payload[9] (caps_invalidated) >= 2          ← C_A2 + C_B comptées
```

### P6
```
CompensationOpen  (0x11) dans le log SCHEDULER_ID [0xFF;16]
CompensationClose (0x12) dans le log SCHEDULER_ID [0xFF;16]
→ journal de compensation complet (récupérable si crash entre les deux)
```

---

## Ce qui n'est PAS testé

| Hors-scope | Raison |
|------------|--------|
| Crash réel entre 0x11 et 0x12 | SEF-4/ICSR — crash-injection feature, hors périmètre R1 |
| P3 sous tempête (traçabilité lookup) | UC-7 (S14) — test indépendant |
| Révocation de 3+ niveaux | S17 — O(depth) déjà couvert |
| seL4 | D7 — substrate requirement |

---

## Invariant ADR-0001 observé

> P4 (check(B,C_B)==false) tient même quand P2 (rollback) et P6 (WaitingInference+compensation)
> sont stressés simultanément. Aucune tension observée qui invaliderait l'ordre P4≻P2≻P6.

Si une future tension était observée (ex. P4 violerait P2 ou P6 sous charge extrême), ce
scénario est le candidat naturel à un amendement d'ADR. Cf. ADR-0001 §Conséquences.

---

## Comment relancer

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s35_storm_p2_p4_p6_arbitrage --exact --nocapture
# Expected: ok — P4≻P2≻P6 tient, C_B révoquée, InferenceCancelled, CompensationOpen/Close
```

---

## Références

- **ADR-0001** — Ordre d'arbitrage P4 ≻ P2 ≻ P3 ≻ P6 ≻ P5 ≻ P1.
- **ADR-0007** — Révocation par rollback : `revoke_owned_after` + cascade O(depth).
- **ADR-0019 §Q5.1** — `agent_infer` + WaitingInference + annulation.
- **ADR-0024** — Journal de compensation CompensationOpen/Close (P6).
- `poc/scenarios/S16-infer-cancel-toctou/` — TOCTOU WaitingInference seul.
- `poc/scenarios/S17-rollback-cap-cascade/` — P2×P4 cascade seul.
- `poc/runtime/src/lib.rs::tests::s35_storm_p2_p4_p6_arbitrage` — test.
