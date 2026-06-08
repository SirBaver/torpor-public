# SEF-9 — Axe 1 : confused-deputy entre isolation et log d'audit (ADR-0050 §D3)

**Date :** 2026-05-30
**Test :** `poc/runtime/src/lib.rs::tests::sef9_audit_masking_under_flood`
**Exécution :** `CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime sef9_audit_masking_under_flood -- --nocapture`
**Verdict :** **1a INTACTE · 1b ÉCHOUE (audit masqué) — finding confirmé.**

---

## Cible (gate SEF-8)

Le gate a classé **P4-audit = PROXY** : le critère spec/02 §P4 « 100% des tentatives d'accès non autorisées sont enregistrées dans le log causal » n'est testé (S9) qu'avec 20 refus, **sous** le rate-limit `0x14` de 100/agent/1s (`actor.rs:829-895`). L'axe 1b attaque ce régime non testé : un adversaire peut-il rendre un refus malveillant **non attribuable** en saturant le rate-limit ?

## Distinction des deux sous-cibles (ADR-0050 §D3)

- **1a — isolation P4** : la cap est-elle accordée à tort / contournée ? Oracle = valeur de retour + état des caps.
- **1b — fidélité du log d'audit** : un refus *tenté* reste-t-il attribuable dans le log ? Oracle = **témoin hors-bande** au point de décision (`cap_denied_witness`, poussé dans `emit_cap_denied` **avant** le rate-limit). `check()` étant pur, un refus ne laisse aucune trace d'état — le témoin hors-bande est le **seul** observable constructible des tentatives, hors du log lui-même.

## Attaque

1. Agent **sans aucune capability** → tout `agent_store_get` est refusé.
2. **Flood** : 101 `get` sur `"bn"` → counts 1..101. Les 100 premiers loggés **avec** resource ; le 101ᵉ **agrégé** (rate_limited=0x01, resource omise).
3. **Refus malveillant** : 1 `get` sur `"secret"` → count 102 → **silencé** (jamais loggé).

## Oracle et résultat

| Observable | Contenu | Source |
|------------|---------|--------|
| Témoin hors-bande (vérité-terrain) | `{"bn", "secret"}` | `cap_denied_witness`, avant rate-limit |
| Log d'audit `0x14` (resources attribuables) | `{"bn"}` | parse des événements non-agrégés |
| **Écart (masqué)** | **`{"secret"}`** | `witness \ log` |

- **1a (isolation) INTACTE** : le `get "secret"` a retourné **-1** (refusé). La cap n'est jamais accordée à tort. P4-isolation tient.
- **1b (fidélité audit) ÉCHOUE** : `"secret"` est dans le témoin (tenté) mais **absent du log** (silencé par le rate-limit). L'écart est non vide → **le refus malveillant est non attribuable**.

```
SEF-9 axe 1b : audit MASQUÉ — masked={"secret"} (1a isolation intacte : secret refusé)
```

## Interprétation (ne PAS sur-vendre — ADR-0050 §D3)

**Ce n'est PAS une violation de P4-isolation.** La cap est correctement refusée ; l'adversaire ne gagne aucun accès. C'est un défaut d'**observabilité** : le mécanisme anti-flood-log (défense contre un DoS du log) devient l'**angle mort** de l'audit. Un adversaire ne peut pas *escalader*, mais il peut **cacher la trace** de ses tentatives en les noyant sous du bruit bénin.

Confused-deputy confirmée : deux mécanismes de défense (rate-limit anti-flood ↔ complétude d'audit) ne sont **pas indépendants** — le premier désarme le second.

## Conséquences

1. **Correction spec/02 §P4 (déclencheur axe 1b atteint)** : le critère « 100% loggé » est **faux sous flood**. Reformulation grounded sur cet exploit : « 100% des refus loggés **jusqu'au rate-limit ; au-delà, agrégation (resource omise) puis silence — par design anti-DoS** ». Amendement spec = décision architect (à porter avec ce verdict).
2. **Décision design ouverte (architect)** : faut-il *corriger* le défaut (ex. : préserver un ensemble borné de resources distinctes refusées même sous agrégation, pour ne jamais perdre l'attribution d'une resource nouvelle), ou *accepter* la limite et la documenter ? L'exploit montre que l'agrégation actuelle (cap_id + count, sans resource) perd l'information de sécurité la plus utile (quelle resource). Une agrégation **par resource** lèverait le masquage sans réintroduire le DoS de log.

## Instrumentation (témoin hors-bande)

`AgentState::cap_denied_witness: Option<Arc<Mutex<Vec<CapDeniedAttempt>>>>` — `None` en production (zéro impact, aucun changement de comportement), `Some` en harness. Poussé en tête d'`emit_cap_denied`, avant toute logique de rate-limit. C'est la réalisation de l'« oracle au point de décision » exigé par ADR-0050 §D3 (l'oracle « état des caps » étant structurellement incapable de falsifier 1b).

## Statut après correctif (ADR-0051 §D2, 2026-05-30)

**Corrigé (#6).** Le rate-limit `0x14` agrège désormais **par resource** (`cap_denied_resources`, ensemble borné à 32 distinctes/fenêtre, `actor.rs`) : une resource **neuve** refusée reste attribuée AVEC sa resource même sous flood, sans réintroduire le DoS du log (taille bornée). Le test `sef9_audit_masking_under_flood` est devenu un **régression-test** : il vérifie désormais que `"secret"` est attribuable (`masked={}`). Masquage levé. L'amendement spec/02 §P4 reflète l'énoncé rehaussé (« attribution préservée pour tout ensemble borné de resources distinctes nouvelles »).

## Références

- `decisions/0050-campagne-mise-a-lepreuve.md` §D3 (axe 1a/1b, oracle hors-bande), §Pièges n°3
- `poc/scenarios/SEF-8-soundness-gate/VERDICT.md` (P4-audit = PROXY — cible)
- `poc/runtime/src/actor.rs:829-895` (rate-limit `0x14`), `cap_denied_witness` (instrumentation)
- `poc/runtime/src/lib.rs::tests::sef9_audit_masking_under_flood`
- `lab/LESSONS.md` L88 (confused-deputy rate-limit ↔ audit)
