# ADR-0007 — Invalidation des capabilities lors d'un rollback

**Date :** 2026-05-13  
**Statut :** Acceptée

---

## Contexte

Lorsqu'un rollback restaure l'état mémoire à un snapshot passé, les agents actifs se retrouvent dans un état incohérent : ils détiennent des capabilities valides qui ont été émises *après* le timestamp du snapshot, donc dans un état qui n'existe plus. Leur prochain accès à une ressource namespaced s'appuie sur un contexte stale.

Avant cette décision, le rollback ne faisait que restaurer la table `memory` — les capabilities restaient intactes. Un agent pouvait continuer à lire ou écrire dans un namespace dont l'état avait été effacé, sans aucun signal de rupture de contexte.

L'architecture capabilities (ADR-0005) est déjà en place depuis Phase 4. Tout accès namespaced passe par le mécanisme grant/check/revoke. Ce mécanisme devient naturellement le vecteur de notification de rupture de contexte.

## Décision

Lors de tout rollback, on révoque automatiquement toutes les capabilities dont `issued_at > snapshot.timestamp`. Les agents concernés reçoivent un 403 `capability_denied` à leur prochaine requête — signal explicite et cohérent avec le comportement attendu d'une révocation.

## Alternatives considérées

| Alternative | Avantages | Inconvénients | Raison du rejet |
|-------------|-----------|---------------|-----------------|
| **A — Epoch de rollback** | Zéro overhead, signal passif lisible dans `/health` | Signal optionnel : les agents qui ne lisent pas l'epoch passent à travers silencieusement | Trop silencieux — les agents continuent à opérer sans savoir que leur contexte est invalide |
| **B — Invalidation des caps post-snapshot** *(retenue)* | — | — | Signal actif via le mécanisme existant ; cohérent avec ADR-0005 |
| **C — Marquage des actions "zombifiées"** | Log reste cohérent et interrogeable | Nouveau primitif, overhead lecture sur toutes les requêtes log | Complexité non justifiée — le log est append-only et ne doit pas acquérir de sémantique de validité |

## Conséquences

**Positives :**
- Les agents reçoivent un signal explicite (403) plutôt qu'un comportement silencieusement incorrect
- La réponse `POST /rollback` inclut `caps_revoked` — auditabilité immédiate
- Le mécanisme réutilise `revoke_capability` existant — la lazy chain check continue à fonctionner pour les dérivées

**Négatives / coûts acceptés :**
- Les agents sans capabilities (accès ouvert) ne sont pas notifiés. Acceptable : en phase 4+, tout accès namespaced sensible est géré par caps. L'accès ouvert = données non protégées = hors périmètre.
- Un agent dont la cap est révoquée doit demander une nouvelle cap à son émetteur. Ce flux de re-grant n'est pas encore formalisé (D3 partiellement résolu — la notification existe, le workflow de récupération reste à définir).

**Neutres / à surveiller :**
- La sémantique temporelle de `issued_at` est celle du serveur (horloge Docker). Si deux caps ont `issued_at == snapshot.timestamp` à la milliseconde près, le comportement dépend de la comparaison stricte `>`. Acceptable pour la phase actuelle.
- Si un rollback est lui-même rollbacké (rollback du rollback), les caps révoquées ne sont pas restaurées. Les caps sont immuables une fois révoquées — cohérent avec le modèle append-only du log causal.

## Références

- ADR-0005 — Design capabilities et révocation (mécanisme réutilisé)
- ADR-0006 — Modèle de supervision (contexte des rollbacks en supervision asymétrique)
- `lab/daemon/capabilities.py::revoke_post_snapshot_caps()`
- `lab/daemon/primitives.py::rollback_to_snapshot()`

---

*Format inspiré de MADR (Markdown Architecture Decision Records) — [Nygard 2011] "Documenting Architecture Decisions"*
