# Guide opérateur — Démonstrateur TUI (`demo-tui`)

Guide pratique : **quelle commande lancer**, **quelle touche fait quoi**, et **ce que `--live` change**.
Pour le texte à dire pendant la démo, voir `docs/demo/demo-tui-script.md` (script complet) et la narration parlée.

---

## 1. Les deux modes (même binaire)

| Mode | Drapeau | Inférence | Quand l'utiliser |
|---|---|---|---|
| **Rejeu** (défaut) | *(aucun)* | Réponses LLM en conserve (instantané, déterministe) | Répétition, présentation sans aléa ni réseau |
| **LIVE** | `--live [modèle]` | Appels réels à Ollama | Montrer que les agents appellent un vrai LLM |

Le mode affiché est écrit en **permanence** dans l'en-tête de l'écran : `mode: rejeu` ou `mode: LIVE (Ollama)`.

> **Ce que `--live` change — et ce qu'il ne change PAS.**
> `--live` n'affecte **que** les deux premiers temps (reviewer → judge) : ils attendent une vraie réponse du LLM (≈ quelques secondes ; le verdict REJECT/APPROVE peut varier d'un run à l'autre).
> La **falsification `[t]`**, le **rollback `[r]`** et l'**intrus `[x]`** sont **strictement identiques** en rejeu et en live : ils agissent sur le **log causal** et les **capabilities**, pas sur l'inférence. Le contrôle des effets ne dépend pas du LLM.

---

## 2. Prérequis (mode LIVE uniquement)

Le mode rejeu n'a **aucun** prérequis LLM. Pour le live :

```bash
# 1. Ollama doit tourner et exposer un modèle
ollama list                       # doit lister un modèle, ex. llama3.2:3b
curl -s http://localhost:11434/api/tags   # doit répondre (serveur up)

# si rien n'est installé :
ollama pull llama3.2:3b           # ~2 Go, modèle par défaut du binaire
```

État actuel de cette machine (vérifié 2026-06-06) : `llama3.2:3b` et `mistral:7b-instruct` présents, serveur up. **Rien à installer.**

---

## 3. Lancement (copier-coller)

Tout se fait **depuis le dossier `poc/`** (le binaire cherche les WASM en chemin relatif `target/wasm32-unknown-unknown/release/examples/`).

### Étape unique préalable — compiler les agents WASM

```bash
cd poc
cargo build --target wasm32-unknown-unknown -p agent-sdk --release \
  --example code_reviewer --example severity_judge \
  --example multi_turn --example data_accessor \
  --example task_step --example incident_aggregator
```

(`task_step` + `incident_aggregator` ne sont requis que pour les scènes `mission-resume`
et `incident` ; sans eux, seule la scène `effects` démarre.)

### Lancer en REJEU (défaut)

```bash
cd poc
CXXFLAGS="-include cstdint" \
  cargo run -p os-poc-runtime --features demo-tui --bin demo-tui --release
```

### Lancer en LIVE (Ollama réel)

```bash
cd poc
CXXFLAGS="-include cstdint" \
  cargo run -p os-poc-runtime --features demo-tui --bin demo-tui --release -- --live
```

### Lancer en LIVE avec un autre modèle

```bash
# n'importe quel modèle présent dans `ollama list`
... --bin demo-tui --release -- --live mistral:7b-instruct
```

> `CXXFLAGS="-include cstdint"` est obligatoire pour compiler RocksDB sous GCC récent (sinon erreur `cstdint`). Pas de `CFLAGS`.

### Choisir une scène — `--scene`

Le binaire expose **quatre scènes** (défaut `effects`). Chacune affiche son **régime** en
permanence à l'écran. `--live` (Ollama) ne s'applique qu'à `effects`, `mission-resume` et
`incident` ; `swarm` utilise un backend simulé.

**Cellule autonome** (compile les WASM puis lance la scène — un seul copier-coller). Le build
WASM est idempotent : instantané aux exécutions suivantes. Change juste le nom de scène à la
fin (`effects` / `mission-resume` / `incident` / `swarm`), ajoute `--live` au besoin.

```bash
cd "$(git rev-parse --show-toplevel)/poc" && \
cargo build --target wasm32-unknown-unknown -p agent-sdk --release \
  --example code_reviewer --example severity_judge --example multi_turn \
  --example data_accessor --example task_step --example incident_aggregator && \
CXXFLAGS="-include cstdint" \
  cargo run -p os-poc-runtime --features demo-tui --bin demo-tui --release -- --scene mission-resume
```

Variantes (remplace la dernière ligne de la cellule ci-dessus) :

```bash
  ... -- --scene effects                  # défaut, 4 temps forts
  ... -- --scene incident                 # triage fan-out/fan-in
  ... -- --scene swarm                     # ordonnancement borné + densité (--live sans effet)
  ... -- --scene mission-resume --live     # mission en Ollama réel (effects/incident aussi)
```

| Scène | Ce qu'elle montre | Touches | Régime affiché |
|---|---|---|---|
| `effects` | reviewer→judge + falsification/rollback/intrus | `Espace` `t` `r` `x` `d` `q` | R1 (effets) |
| `mission-resume` | tâche 4 étapes, interruption *simulée*, reprise sans recompute | `Espace` (lancer) `d` (preuve) `q` | R1 (traçabilité P3) |
| `incident` | fan-out 3 spécialistes → fan-in agrégateur (DAG) | `Espace` (lancer) `d` (preuve) `q` | R1 (DAG B-light) |
| `swarm` | admission bornée (C2) puis éviction/réveil (densité) | `Espace` (lancer) `e` (évincer) `w` (réveiller) `q` | mécanisme (R2 non mesuré) |

> **Honnêteté par scène** (affichée en pied d'écran) : `mission-resume` = interruption
> SIMULÉE → P3, **pas** P6 ni durabilité. `incident` = B-light mono-tenant (ADR-0036).
> `swarm` = **mécanisme, pas une mesure** : N à l'écran ≠ N soutenables, aucun ~100 agents/s.

### Walkthrough seL4 — isolation forte (hors TUI)

Build + boot QEMU AArch64 démontrant le **W^X matériel** (écriture sur page RX → `vm fault`
seL4). Prérequis : Docker + image locale `rust-root-task-demo`. Le build est conteneurisé
(~1 GB de `target/`, nettoyé en fin de run sauf `--keep`).

```bash
# depuis la racine du dépôt
bash poc/sel4-hello/demo-isolation.sh            # W^X matériel (C.10)
bash poc/sel4-hello/demo-isolation.sh --with-c7  # + I4 non-interférence VSpace (C.7)
bash poc/sel4-hello/demo-isolation.sh --keep     # conserver les artefacts de build
```

> Transcript de référence rejouable (sans rebuild) : `docs/demo/sel4-transcripts/c10-wx-phaseA.txt`.
> Verdict d'**isolation**, pas de performance — latence non recevable sur QEMU (ADR-0046) ; D7.

---

## 4. Table des touches (les « commandes » de la démo)

Une fois dans l'interface :

| Touche | Action | Propriété montrée | Dépend du LLM ? |
|---|---|---|---|
| `Espace` | Avance le pipeline d'un cran : 1er appui → le **reviewer** rend son rapport ; 2e appui → le **judge** lit ce rapport et rend un verdict (l'arête cross-agent = un hash) | Traçabilité causale (P3 — intégrité, B-light) | **Oui** en live (vraie inférence) |
| `d` | Ouvre / ferme le **panneau de preuve** (drill) : hashes 64-hex complets, parent = hash exact du rapport, payload décodé, **+ le rapport complet du reviewer (les causes) et le verdict du juge** | Content-addressing + raisonnement LLM | non (lecture du log) |
| **`t`** | **FALSIFICATION** : altère 1 octet d'une entrée écrite, recalcule l'`action_id`, montre `stored ≠ recalculé` et le juge devient **ORPHELIN**. Re-appui = rétablit. | Tamper-evident (P3 — intégrité) | **non** |
| `r` | **ROLLBACK** : annule atomiquement l'état de l'agent `memo` (`Message::Rollback`), trace un `SchedulerRollback (0x0B)`. P2 passe au vert. | Rollback atomique (P2) | **non** |
| `x` | **INTRUS** : l'agent `rogue` tente d'accéder à `confidential/salaires_2024` (hors de sa capability `reports`). Bloqué à la frontière → `CapabilityDenied (0x14)`. P4 s'allume. | Isolation par capabilities (P4) | **non** |
| `q` | Quitter (restaure le terminal) | — | — |

**Séquence de démo recommandée :** `Espace` → `Espace` → `d` (montrer la preuve) → `d` (refermer) → **`t`** (l'effraction) → `t` (rétablir) → `r` → `x` → `q`.

---

## 5. Le cas simple, en live

Le code soumis au reviewer est volontairement une faille classique — un login vulnérable à l'**injection SQL** :

```python
def login(db, user, pwd):
    sql = "SELECT * FROM users WHERE name='" + user + "'"
    row = db.execute(sql).fetchone()
    return row if row and row['password'] == pwd else None
```

En live, `llama3.2:3b` repère réellement la faille → le reviewer la signale → le judge rend (le plus souvent) **REJECT**. La narration affichée en bas lit le **vrai** verdict du run : si le LLM répond APPROVE, l'écran l'affiche tel quel — on ne maquille rien.

---

## 5 bis. Scénario « vérificateur tiers » — la falsification crédible

La falsification `[t]` dans le TUI est pédagogique mais **téléphonée** : c'est le même programme qui casse et qui détecte, et il ne modifie même rien sur disque (il recalcule un hash en mémoire). Pour une démo *crédible* devant un public technique, on sépare les rôles en **trois programmes distincts** :

1. **`demo-tui`** (l'écrivain) écrit le log causal sur disque, à un chemin **stable** : `demo-work/log`.
2. **`log-tamper`** (l'attaquant) modifie une entrée déjà écrite — *valeur mutée, clé inchangée*.
3. **`log-verify`** (l'auditeur) rouvre le log à froid, dans un process distinct, et recalcule `clé == SHA256(valeur)`. Il ignore tout de ce qui a été touché : il le retrouve seul.

Le log est **content-addressé** (comme les objets Git) : la clé d'une entrée *est* le SHA-256 de son contenu. Modifier le contenu sans recalculer la clé rend la divergence détectable par n'importe quel tiers.

### Séquence (depuis `poc/`)

```bash
# 0. compiler les deux outils (une fois)
CXXFLAGS="-include cstdint" cargo build --release \
  -p os-poc-runtime --bin log-verify
CXXFLAGS="-include cstdint" cargo build --release \
  -p os-poc-runtime --bin log-tamper --features demo-tamper

# 1. lancer la démo, construire le DAG (Espace, Espace…), puis QUITTER avec q
#    (quitter libère le verrou RocksDB — indispensable avant l'étape 2)
CXXFLAGS="-include cstdint" cargo run --release \
  -p os-poc-runtime --bin demo-tui --features demo-tui

# 2. AUDIT INITIAL — doit être intègre (exit 0)
target/release/log-verify

# 3. L'ATTAQUANT corrompt une entrée sur disque
target/release/log-tamper

# 4. RE-AUDIT par le tiers — détecte seul la falsification (exit 1)
target/release/log-verify
```

Étape 4 attendue :
```
  ✗ FALSIFICATION DÉTECTÉE
    [hash] clé   08f60f1798db2a44…  stockée sous ce hash
           recalc 7751933c4bd7b51b…  ≠ clé → valeur modifiée après écriture
```

`log-verify` pointe **exactement** l'`action_id` que `log-tamper` a touché, sans l'avoir jamais su.

### Codes de sortie (pour scripter une démo / un test)

| Code | `log-verify` | `log-tamper` |
|---|---|---|
| 0 | log intègre | corruption écrite |
| 1 | **falsification détectée** | — |
| 2 | erreur I/O / chemin absent | erreur I/O / cible introuvable |

Options : `log-verify [--db <path>]` · `log-tamper [--db <path>] [--key <hex64>] [--byte <n>]` (défaut DB : `demo-work/log`).

### Ce que ce scénario prouve — et ce qu'il ne prouve PAS

**Prouve :** un tiers indépendant détecte (a) la mutation d'une valeur sous clé inchangée (`clé ≠ SHA256(valeur)`) et (b) toute référence parent pendante. Détection **par un process distinct**, à froid, sans état partagé avec l'écrivain.

**Ne prouve PAS — à ne jamais sous-entendre :**
- **Réécriture cohérente d'un sous-arbre** par un adversaire ayant un accès en écriture (re-keying d'une feuille) : exigerait un chaînage Merkle de tête *signé*. Hors portée.
- **Bit-rot du stockage** (corruption physique d'un fichier SST) : couvert par les checksums de bloc RocksDB — **un mécanisme différent du nôtre**. Ne pas confondre.
- **Troncature** : supprimer la dernière entrée laisse un log cohérent plus court ; non détecté ici.

> Garde-fou : c'est toujours du **régime R1** (contrôle des effets, P3 — intégrité / tamper-evidence). C'est une *mise en scène lisible* d'une propriété déjà validée par oracle (S32 forgerie causale, SEF-13), pas une propriété nouvelle. (Rappel Q1 : « P3a » désigne la **latence** de lookup, jamais l'intégrité — ne pas confondre.)

---

## 5 ter. Trois variantes « anti-téléphoné »

Pour désamorcer le soupçon « tout est préparé d'avance », trois modes supplémentaires retirent à l'opérateur le contrôle du résultat.

### a) Code fourni par le public — `demo-tui --code <fichier>`

Un spectateur fournit *son* code ; le reviewer le relit. Le verdict du LLM devient réellement imprévisible — mais trace, falsification, rollback et isolation tiennent quand même. **À coupler avec `--live`** (sinon, en rejeu, la réponse en conserve ignore le contenu du fichier) :

```bash
cargo run --release -p os-poc-runtime --bin demo-tui --features demo-tui -- \
  --live --code /chemin/vers/le_code_du_public.py
```

> « Apportez n'importe quel code. Je ne sais pas ce que le LLM va en dire — et ça n'a pas d'importance : le contrôle des effets est le même. » Fichier absent → sortie propre (exit 2).

**Échantillons prêts à l'emploi — `docs/demo/demo_samples/`** (5 cas choisis pour couvrir des verdicts différents) :

| Fichier | Contenu | Angle de démo |
|---|---|---|
| `script_1.py` | Injection de commande OS — `subprocess(…, shell=True)` + f-string (CWE-78) | Faille nette ; REJECT attendu |
| `script_2.py` | Désérialisation non sûre — `pickle.loads` sur données externes (CWE-502, RCE) | Classique, en général bien détectée |
| `script_3.py` | Hash faible pour mot de passe — `hashlib.md5` (CWE-327/916) | Plus subtil : teste si le modèle connaît la bonne pratique crypto |
| `script_4.py` | **Pas de faille de sécurité** : tourne, mais style déplorable + piège latent (argument par défaut mutable `acc=[]`) | « Moche » ≠ « vulnérable » : le verdict peut surprendre → bascule idéale vers le levier (c) « LLM faillible » |
| `script_5.py` | **Erreur de syntaxe** (`def calcule(a, b)` sans `:`) | Le code ne s'exécute même pas — que fait le LLM ? Test de robustesse |

**Commandes de test (copier-coller).** Tout se lance **depuis `poc/`** (les WASM sont cherchés en chemin relatif) ; les scripts sont donc référencés en `../docs/demo/demo_samples/…`.

```bash
cd poc

# (une seule fois) compiler les 4 agents WASM + le binaire de démo
cargo build --target wasm32-unknown-unknown -p agent-sdk --release \
  --example code_reviewer --example severity_judge \
  --example multi_turn --example data_accessor
CXXFLAGS="-include cstdint" cargo build --release \
  -p os-poc-runtime --bin demo-tui --features demo-tui

# tester un échantillon en LIVE (remplacer le numéro par 1..5)
CXXFLAGS="-include cstdint" cargo run --release \
  -p os-poc-runtime --bin demo-tui --features demo-tui -- \
  --live --code ../docs/demo/demo_samples/script_1.py
```

Les cinq, à lancer un par un (Espace → Espace → lire le verdict → `q`) :

```bash
# 1. injection de commande (CWE-78)      — REJECT attendu
... -- --live --code ../docs/demo/demo_samples/script_1.py
# 2. pickle.loads non sûr (CWE-502)      — REJECT attendu
... -- --live --code ../docs/demo/demo_samples/script_2.py
# 3. md5 sur mot de passe (CWE-327)      — subtil, le petit modèle peut le rater
... -- --live --code ../docs/demo/demo_samples/script_3.py
# 4. sain mais illisible (acc=[] mutable) — « moche » ≠ « vulnérable »
... -- --live --code ../docs/demo/demo_samples/script_4.py
# 5. erreur de syntaxe (def sans ':')     — test de robustesse du prompt
... -- --live --code ../docs/demo/demo_samples/script_5.py
```

(`...` = `CXXFLAGS="-include cstdint" cargo run --release -p os-poc-runtime --bin demo-tui --features demo-tui`)

> Rappel honnêteté : la démo **n'exécute pas** ces scripts — le reviewer LLM les lit comme du texte. Le verdict reflète l'avis réel du modèle (variable d'un run à l'autre, surtout sur `script_3`/`script_4`). C'est précisément ce qui rend le « code du public » crédible : rien n'est scripté.

> **Pour voir _pourquoi_ le LLM a tranché**, appuie sur `[d]` après le verdict : le panneau PREUVE déroule le **rapport complet du reviewer** (les causes) puis le verdict du juge. Indispensable pour distinguer un REJECT *justifié* (vraie faille citée) d'un REJECT *halluciné* (faille inexistante) — la frontière du levier (c).

### b) Falsification aveugle — `log-tamper --blind`

L'opérateur ne vise rien : l'entrée **et** l'octet sont tirés à l'horloge et **cachés**. L'auditeur retrouve seul l'emplacement.

**Prérequis :** un log doit déjà exister dans `poc/demo-work/` — lance une démo (§3,
scène `effects`), construis le DAG (`Espace`×2) puis quitte avec `q`. Ensuite, **cellule
autonome** (build des deux outils + falsification aveugle + ré-audit, un seul copier-coller) :

```bash
cd "$(git rev-parse --show-toplevel)/poc" && \
CXXFLAGS="-include cstdint" cargo build --release -p os-poc-runtime --bin log-tamper --features demo-tamper && \
CXXFLAGS="-include cstdint" cargo build --release -p os-poc-runtime --bin log-verify && \
target/release/log-tamper --blind && \
target/release/log-verify     # révèle l'entrée touchée, exit 1
```

> « Je ne choisis pas ce que je casse. Le vérificateur, qui n'était pas dans la combine, le trouve quand même. » (Le public peut aussi *dicter* la cible : `log-tamper --key <hex64> --byte <n>`.)

### c) Assumer l'erreur du LLM — `demo-tui --llm-wrong`

Rejeu d'un LLM qui **rate** la faille : le reviewer ne voit rien, le juge **APPROVE** du code vulnérable. L'en-tête affiche `scénario: LLM faillible`.

```bash
cargo run --release -p os-poc-runtime --bin demo-tui --features demo-tui -- --llm-wrong
```

> « Le LLM s'est trompé. Le système **ne corrige pas** sa décision — ce n'est pas son rôle. Il la rend **traçable et attribuable** (au juge, dans le log [d]). Contrôler les *effets* n'est pas garantir la *justesse sémantique* du modèle (frontière LLM = non-objectif). »

C'est la variante la plus désarmante pour un public technique : on concède la limite au lieu de la masquer.

---

## 6. Dépannage

| Symptôme | Cause | Remède |
|---|---|---|
| `…​.wasm manquant` au démarrage | agents WASM non compilés ou lancé hors de `poc/` | refaire l'étape §3 et lancer **depuis `poc/`** |
| Erreur de compilation `cstdint` | RocksDB sous GCC récent | préfixer `CXXFLAGS="-include cstdint"` |
| En live, `Espace` reste bloqué longtemps | inférence Ollama en cours (normal) ou serveur down | vérifier `curl …/api/tags` ; timeout interne 30 s |
| Écran vide / illisible | terminal trop petit | agrandir la fenêtre (≥ ~100 colonnes) |
| Verdict APPROVE au lieu de REJECT en live | aléa du LLM sur un petit modèle | refaire `Espace`, ou utiliser `mistral:7b-instruct`, ou rester en rejeu pour une démo reproductible |

---

## 7. Garde-fou (à ne pas oublier en présentation)

Le mode **live prouve que de vrais agents appellent un vrai LLM sur un cas réel** — pas une performance d'inférence ni une validation de débit (garde-fou **F1** : une démo n'est pas un benchmark). Régime de la démo : **R1 — contrôle des effets** (P2/P3/P4/P6). Aucune propriété R2 (densité, pool, déterminisme) n'est revendiquée ici. Substrat : **Linux** (les verdicts ne transfèrent pas à seL4, D7).

---

### Références
- Binaire : `poc/runtime/src/bin/demo_tui.rs`
- Script de présentation : `docs/demo/demo-tui-script.md`
- Agents WASM : `poc/agent-sdk/examples/{code_reviewer,severity_judge,multi_turn,data_accessor,task_step,incident_aggregator}.rs`
- Walkthrough seL4 : `poc/sel4-hello/demo-isolation.sh` · transcript `docs/demo/sel4-transcripts/c10-wx-phaseA.txt`
