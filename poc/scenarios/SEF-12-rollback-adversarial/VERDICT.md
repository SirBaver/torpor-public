# SEF-12 — Rollback adversarial (P2, campagne ADR-0053 §D-P2)

**Date :** 2026-05-30
**Binaire :** `sef12-runner` (release)
**Verdict global : PASS (3/3)**

---

## Gate (Q1) — N max entre commit barriers

`session_max_actions = 10 000` (actor.rs). La spec revendique ≤ 100 ms pour depth = 100.
À ~34 µs/saut (SEF-2 mesuré), depth = 100 → 3,4 ms << 100 ms.
**V2.1 (borne murale longue chaîne) TOMBE** : la spec n'est jamais approchée dans son périmètre déclaré.

---

## V2.2 — rollback² (naviguation jonction nouvelle-branche→chaîne-originale) : PASS

**Scénario :**
1. 100 actions → snapshot à seq=49 (`hash_at_50`) et seq=29 (`hash_at_30`)
2. Rollback-1 → target_seq=49 (restaure `hash_at_50`)
3. 10 nouvelles actions sur la nouvelle branche (seq=101..111)
4. Rollback-2 → target_seq=29 (navigue jonction : seq 111→101→49→48→...→29)

**Oracle non-trivial :** P-δ₂ = l'action suivant rollback-2 a `hash_before == hash_at_30` (hash original, pas de la branche).

**Propriétés vérifiées :**
- P-α₂ : `SchedulerRollback#2.hash_after == hash_at_30` → **pass**
- P-β₂ : `target SnapshotHeader.seq == 29` → **pass**
- P-γ₂ : `payload.target_seq == 29` → **pass**
- P-δ₁ : `post-rb1.hash_before == hash_at_50` → **pass**
- P-δ₂ : `post-rb2.hash_before == hash_at_30` → **pass**

**Finding :** `rollback_path` navigue correctement la jonction entre la nouvelle branche (snapshots seq=101..111) et la chaîne originale (seq=49→29). Aucun état déchiré, aucun parent pendant.

---

## V2.3 — rollback + flood immédiat (FIFO ordering) : PASS

**Scénario :** 50 actions → rollback à k=25 → 30 Data messages envoyés immédiatement.

**Propriétés vérifiées :**
- P-δ : premier post-rollback (par seq) a `hash_before == hash_at_25` → **pass**
- P-ordering : 30 actions post-rollback forment une chaîne cohérente → **pass**
- P-rb : `SchedulerRollback.hash_after == hash_at_25` → **pass**

**Note :** `query_by_agent_range` trie par `(ts_ms, action_id)`, pas par `seq`. Les chaînes doivent être vérifiées après tri par seq — piège de test documenté.

---

## V2.4 — liveness sous charge (80 msgs en vol) : PASS

**Scénario :** 50 actions → rollback → 80 Data messages en charge.

**Propriétés vérifiées :**
- P-liveness : rollback complète sous charge → **pass** (6 ms observé)
- P-rb : `SchedulerRollback.hash_after == hash_at_25` → **pass**
- P-δ : premier post-rollback (par seq) a `hash_before == hash_at_25` → **pass**

---

## Verdict P2 (campagne adversariale)

P2 tient sous les trois vecteurs constructibles (V2.2/V2.3/V2.4).
V2.1 (borne murale longue chaîne) non falsifiable dans le périmètre spec (depth ≤ 100 → 3,4 ms << 100 ms).

**P2 : PASS (campagne adversariale, substrat Linux PoC — non transférable seL4, ADR-0050 §D7)**

## Références

- `decisions/0053-cadrage-campagne-p2-p3-p5.md` §D-P2 (vecteurs, oracle G-P2)
- `poc/runtime/src/bin/sef12_runner.rs`
- `poc/store/src/lib.rs:137-170` (`rollback_path` — O(depth), traversée jonction)
- `poc/runtime/src/actor.rs:2316-2430` (handler `Message::Rollback`)
- SEF-2 (`poc/scenarios/S7-rollback-equivalence/`) — oracle de base P-δ
