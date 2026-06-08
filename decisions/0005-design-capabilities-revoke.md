# Design Document — Capabilities et révocation (H-revoke)

**Statut :** Design validé par session d'observation — prêt pour implémentation  
**Hypothèse associée :** H-revoke dans `spec/04-hypotheses.md`  
**Propriété cible :** P4 (Isolation par capabilities), `spec/02-properties.md`

---

## 1. Contexte et objectif

P4 pose que tout accès à une ressource requiert une capability explicite, et que la révocation d'une capability invalide récursivement toutes ses dérivées. C'est la propriété la plus difficile à valider empiriquement : elle ne se vérifie pas par une mesure continue (latence, débit), elle se vérifie par un *scénario d'attaque* — un sous-agent tente d'accéder à une ressource dont sa capability a été révoquée, et le système doit bloquer cet accès.

Ce document tranche cinq questions de design qu'il faut résoudre avant d'écrire la moindre ligne de code. Sans ces décisions, tout prototype de capabilities sera du code jetable.

---

## 2. Questions ouvertes et décisions proposées

### Q1 — Granularité des capabilities

**La question :** À quel niveau de granularité une capability contrôle-t-elle l'accès ? Options :

| Niveau | Exemples | Commentaire |
|--------|---------|-------------|
| Namespace entier | `agent-a/*` | Simple, couvre les cas du lab actuel (namespaces ADR-0004) |
| Clé individuelle | `agent-a/user.name` | Très précis, coût de gestion élevé |
| Opération × namespace | `(read, agent-a/*)` | Couvre l'atténuation de permission décrite dans P4 |
| Opération × clé | `(write, agent-a/user.name)` | Maximal, difficile à gérer en pratique |

**Décision proposée : opération × namespace** — aligne avec l'atténuation deux-dimensions de P4 (permission + portée), compatible avec les namespaces déjà implémentés (ADR-0004), et couvre les cas d'usage réels du lab (un agent lit le namespace shared, écrit dans son namespace privé).

**Format de capability :** `{op: "read"|"write"|"read_write", scope: "namespace/prefix"}`. Exemple : `{op: "read", scope: "shared/"}`.

> **Note d'implémentation (2026-05-15) :** Le poc Rust (`poc/capabilities/src/lib.rs`) utilise `resource: String` avec correspondance **exacte** (`cap.resource == resource`) plutôt qu'un préfixe de scope. La sémantique du poc est donc token exact, pas namespace prefix. Cette divergence est intentionnelle pour la Phase 2 (cas d'usage simples) — si la sémantique prefix devient nécessaire (ex. `"shared/"` couvre `"shared/x"` et `"shared/y"`), elle devra être implémentée dans `check()` avant tout déploiement multi-agent sur namespaces partagés. Le type `CapabilityId` est également `u64` (compteur atomique) dans le poc vs `TEXT PRIMARY KEY` (UUID) dans le schéma SQLite de ce document.

**Validé expérimentalement (session d'observation pré-phase 4, 2026-05-13) :** la granularité namespace est suffisante. L'observation principale : le vecteur d'exfiltration le plus simple n'est pas `memory_read` sur une clé spécifique, c'est `memory_list({})` sans namespace — qui retourne tout le store. La capability `{op: read, scope: "X/"}` doit donc s'appliquer aussi à `memory_list`, pas seulement aux opérations de lecture unitaires. Voir `lab/LESSONS.md` §L11.

**Point d'attention ajouté :** `memory_list` est dans le périmètre des capabilities. Quand un agent appelle `memory_list({})` sans namespace, le système doit filtrer le résultat aux seuls scopes couverts par ses capabilities (option ergonomique) ou refuser et exiger un namespace (option disciplinaire). Décision : option ergonomique pour ne pas rompre les agents existants, mais le filtre est une exigence non négociable.

---

### Q2 — Format des tokens de capability

**La question :** Comment représenter une capability de façon à ce qu'elle soit vérifiable, non-forgeable, et révocable ?

Deux approches principales :

**Option A : token opaque (UUID référencé en DB)**  
Un UUID est émis à la création. La vérification consiste à chercher l'UUID dans la table `capabilities`. La révocation supprime la ligne.

- Pour : simple, révocation immédiate et certaine
- Contre : coût de lookup DB sur chaque accès, pas de vérification offline

**Option B : token signé (JWT ou HMAC)**  
Le token encode `{subject, op, scope, issued_at, expires_at}` et est signé par une clé serveur. La vérification est locale (pas de DB lookup). La révocation nécessite une liste de révocation (blocklist).

- Pour : vérifié sans DB, portable entre nœuds
- Contre : révocation probabiliste (blocklist) ou délai TTL, complexité de gestion de clé

**Décision proposée : option A (token opaque) pour le lab.** Pour H-revoke, la propriété à tester est que la révocation est immédiate et propagée. Option A est la seule qui garantit cela sans TTL. Option B sera pertinente si on vise l'exportabilité inter-nœuds (hors scope du lab actuel).

**Schéma proposé :**
```sql
CREATE TABLE capabilities (
    cap_id      TEXT PRIMARY KEY,  -- UUID
    parent_cap  TEXT REFERENCES capabilities(cap_id),
    subject     TEXT NOT NULL,     -- session_id ou agent_id bénéficiaire
    op          TEXT NOT NULL,     -- "read" | "write" | "read_write"
    scope       TEXT NOT NULL,     -- ex: "shared/" ou "agent-a/"
    issued_at   TEXT NOT NULL,
    issued_by   TEXT NOT NULL,     -- action_id de l'action de délégation
    revoked_at  TEXT,              -- NULL = active, timestamp = révoquée
    revoked_by  TEXT               -- action_id de la révocation
);
```

---

### Q3 — Mécanisme de dérivation

**La question :** Comment un agent délègue-t-il une capability dérivée à un sous-agent, en respectant l'atténuation ?

**Règles d'atténuation (rappel P4) :**
1. La dérivée ne peut pas avoir une portée plus large que la source
2. La dérivée ne peut pas avoir des permissions plus étendues que la source
3. La source peut déléguer une capability qu'elle ne détient pas en propre si elle en détient une plus large — mais jamais l'inverse

**Mécanisme proposé :**

L'orchestrateur appelle un outil `capability_grant` :
```
capability_grant(
    to_subject: "sous-agent-b",
    op: "read",          # ≤ op de la cap source
    scope: "shared/",    # ⊆ scope de la cap source
    parent_cap: "<cap_id de la cap source>",
    expires_in_s: 300    # optionnel, pour les caps exportées
)
```

Vérification côté système :
1. La cap source `parent_cap` est active (non révoquée)
2. L'opération demandée est ≤ l'opération de la source (`read ≤ read_write`, `write ≤ read_write`, jamais `read_write > read`)
3. Le scope demandé est un préfixe du scope de la source (⊆ relation simple sur les strings)
4. L'émetteur (`current_subject`) détient bien la cap source

Si toutes les conditions sont satisfaites, une nouvelle ligne est insérée dans `capabilities` avec `parent_cap` renseigné.

---

### Q4 — TTL pour les capabilities exportées

**La question :** Une capability peut-elle être exportée hors du nœud (transmise à un agent tournant sur un autre nœud, ou sauvegardée dans le store pour être utilisée plus tard) ? Si oui, comment gérer la révocation sur une cap qui n'est plus sous contrôle direct du nœud émetteur ?

**Position pour le lab (scope H-revoke) :** les capabilities restent intra-nœud. Toutes les caps référencent le même store SQLite. La révocation est donc immédiate — il n'y a pas de problème d'export.

**Pour la spec (note vers P4) :** si on veut éventuellement supporter les caps inter-nœuds, il faudra choisir entre :
- TTL court (ex. 30–300s) : la cap expire si le nœud émetteur ne la renouvelle pas, ou si elle est révoquée et que le TTL s'est écoulé
- Blocklist distribuée : vérification online à chaque accès (latence réseau)
- Token signé avec embedded claims + TTL (Option B de Q2)

**Décision pour le lab :** pas de TTL sur les caps intra-nœud. TTL sera documenté comme contrainte de conception pour la phase 4+ si l'architecture devient distribuée.

---

### Q5 — Propagation de la révocation

**La question :** Quand une capability est révoquée, comment les dérivées sont-elles invalidées ?

**Deux modes :**

**Mode eager** : à la révocation de la cap source, le système parcourt récursivement toutes les dérivées (via `parent_cap`) et les révoque immédiatement.
- Pour : révocation instantanée et certaine
- Contre : coût O(N_dérivées), potentiellement lent si l'arbre est profond

**Mode lazy** : la révocation ne touche que la cap source. La vérification à l'accès remonte la chaîne `parent_cap → parent_cap → …` jusqu'à la racine et échoue si l'une quelconque est révoquée.
- Pour : révocation O(1) en écriture
- Contre : vérification d'accès O(profondeur_arbre), et le log causal ne capture pas la révocation des dérivées

**Décision proposée : mode lazy** pour le lab, parce qu'il est plus simple à implémenter et que la profondeur de l'arbre de dérivation dans les scénarios du lab est petite (orchestrateur → sous-agent, rarement plus de 2 niveaux). Le mode eager sera requis si on veut des garanties de révocation inscrites dans le log causal pour chaque dérivée.

> **Amendement (2026-05-15) :** Le poc Rust (`poc/capabilities/src/lib.rs::revoke()`) implémente le **mode eager** — BFS récursif qui supprime immédiatement toutes les caps dérivées via les `delegated` sets. Ce choix a été fait parce que le plafond de ~10K caps documenté en L21 rend le coût O(N) acceptable à l'échelle Phase 2, et parce que la suppression immédiate est plus sûre en environnement non-distribué. Le mode lazy reste la référence conceptuelle pour la Phase 4+ (caps inter-nœuds). Cette décision remplace la décision lazy ci-dessus pour l'implémentation poc actuelle.

**Vérification access (pseudo-code) :**
```python
def check_capability(db_path, cap_id, subject, required_op, required_scope):
    cap = fetch_cap(db_path, cap_id)
    if cap is None or cap["revoked_at"] is not None:
        return False  # révoquée ou inexistante
    if cap["subject"] != subject:
        return False  # mauvais bénéficiaire
    if not op_satisfies(cap["op"], required_op):
        return False  # permissions insuffisantes
    if not scope_covers(cap["scope"], required_scope):
        return False  # portée insuffisante
    if cap["parent_cap"] is not None:
        return check_capability(db_path, cap["parent_cap"], cap["issued_by_subject"], cap["op"], cap["scope"])
    return True  # racine atteinte, cap valide
```

---

## 3. Scénario de test H-revoke

Le test de H-revoke n'est pas un benchmark — c'est un scénario adversarial binaire. Plan :

**Étape 1 — Setup :**
- Orchestrateur O détient `cap_root = {op: read_write, scope: "shared/"}`
- O délègue à sous-agent A : `cap_a = {op: read, scope: "shared/", parent: cap_root}`
- A lit `shared/secret` → succès attendu

**Étape 2 — Révocation :**
- O révoque `cap_root` (ou directement `cap_a`)
- Le système marque `revoked_at` sur la cap révoquée

**Étape 3 — Tentative d'accès post-révocation :**
- A tente de lire `shared/secret` avec `cap_a`
- Résultat attendu : accès refusé (403 ou erreur explicite), log causal enregistre la tentative

**Étape 4 — Vérification dérivée :**
- O crée `cap_b = {op: read, scope: "shared/", parent: cap_root}` *après* la révocation de `cap_root`
- Tentative d'accès avec `cap_b` : doit échouer (parent révoqué)

**Métriques :**
- Accès refusé : 100% des tentatives post-révocation → `False`
- Aucun faux positif : accès autorisé avant révocation → `True`
- Log causal : toute tentative refusée doit apparaître dans `actions` avec type `capability_denied`

---

## 3b. Pré-requis avant Phase 4 : corriger le double-namespace

La session d'observation a révélé un bug dans l'interface outil (voir `lab/LESSONS.md` §L12) : si le modèle passe `namespace="shared"` ET `key="shared/session.goal"`, la clé stockée devient `shared/shared/session.goal`. Ce bug doit être corrigé avant d'implémenter les capabilities, car il crée des clés fantômes qui ne correspondent à aucun namespace attendu et fausseraient les vérifications de scope.

**Fix requis dans `daemon/tools.py::execute_tool`** : avant d'appeler `mem.write_key`, si `namespace` est fourni et que `key` commence par `{namespace}/`, strip ce préfixe. Et dans `_NS_DESC`, ajouter : *"Do NOT include the namespace as a prefix in the key itself — it is prepended automatically."*

---

## 4. Ce qu'on ne fait pas dans cette phase

- Pas de caps inter-nœuds
- Pas de caps sur les messages (uniquement sur les lectures/écritures store)
- Pas de caps sur les appels LLM eux-mêmes (hors scope)
- Pas d'interface LLM pour la gestion des caps dans cette phase — les caps sont créées et révoquées par le code de test, pas par un agent LLM

---

## 5. Prochaine étape

Ce document répond aux cinq questions de design. Avant tout prototypage :

1. Valider Q1 (granularité opération × namespace) sur un cas réel : est-ce suffisant pour le scénario adversarial §3 ?
2. Implémenter le schéma SQL de la table `capabilities` et les fonctions `check_capability` et `grant_capability` dans `daemon/capabilities.py`
3. Brancher `check_capability` dans `execute_tool` avant chaque `memory_read` / `memory_write`
4. Écrire le test smoke P4.1–P4.4 correspondant au scénario §3
5. Mesurer : revocation_propagation_delay (doit être O(1) en mode lazy), false_denial_rate (doit être 0%)
