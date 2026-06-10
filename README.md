# OS-pour-IA

Projet de recherche-conception : qu'est-ce qu'un système d'exploitation conçu pour des agents IA comme utilisateurs principaux ?

Exercice intellectuel rigoureux, à mi-chemin entre spec technique et recherche académique.

---

## Navigation

- [QUICKSTART](QUICKSTART.md) — faire tourner le PoC
- [Spec](spec/) — design document (10 fichiers, P1–P6, plafonds C1–C3)
- [ADRs](decisions/) — décisions d'architecture (ADR-0001–0053)
- [LESSONS](lab/LESSONS.md) — leçons empiriques (L1–L92)
- [TODO](TODO.md) — dettes actives et déclencheurs dormants

---

## Le problème

Les OS contemporains (Linux, Windows, macOS) ont été conçus pour des utilisateurs humains en temps interactif. Leurs hypothèses implicites — temps perçu en millisecondes, état navigué par convention, confiance liée à l'identité, parallélisme borné par la cognition humaine — coûtent cher aux agents autonomes long-running qui les utilisent comme substrat faute d'alternative. Ce projet formule les primitives qu'un OS devrait offrir si les agents IA étaient les utilisateurs premiers.

Le profil cible (profil B) : agent autonome, durée de vie 1h–1 mois, 10⁴–10⁸ actions/lifetime, supervision humaine périodique (heures à jours), spawn de sous-agents. Ni un processus éphémère, ni un service permanent. Détail : `spec/01-vision.md`.

---

## Thèse centrale : trois propriétés mesurables

Un OS conçu pour ce profil peut garantir, par construction, trois propriétés que Linux n'offre qu'au prix de couches applicatives coûteuses :

- **P1a — Densité ×5** : maintenir 5× plus d'agents *dormants* que Linux+containers sur le même hardware (Wasmtime vs Docker+Python). *Mesure d'empreinte au repos uniquement ; la densité active P1b — débit à latence équivalente — n'est pas revendiquée (comparaison abandonnée, non transférable au substrat seL4-natif).*
- **P2 — Rollback transactionnel ≤ 100 ms** : revenir à l'action N°k parmi N en O(depth), borne ≤ 100 ms pour N=500.
- **P3a — Traçabilité causale O(1) ≤ 10 ms** : lookup par `action_id` en p99 ≤ 10 ms sur 10⁸ actions.

Priorité formelle : P4 ≻ P2 ≻ P3 ≻ P6 ≻ P5 ≻ P1 (ADR-0001). La densité (P1) passe en dernier ; la correction fonctionnelle prime.

---

## Convictions architecturales

Ces paris sont faillibles ; les conditions de réfutation sont dans `spec/04-hypotheses.md`.

**Pari 1 — DAG causal, pas arbre (ADR-0003)**  
La causalité réelle entre actions parallèles est un DAG. Chaque action porte `caused_by[]`, liste des parents directs. Validé dès les premières expériences multi-agents (L1).

**Pari 2 — RocksDB LSM, pas SQLite B-tree (ADR-0002, ADR-0011)**  
Pour un log append-only à 10⁸ entrées avec lookup O(1) par clé opaque, le LSM tree de RocksDB est architecturalement correct. Validé par T5 (p99 ≤ 5 ms sur 10⁸ entrées NVMe, ADR-0026).

**Pari 3 — Wasmtime + Tokio, pas Docker (ADR-0002)**  
L'isolation par module WASM + Wasmtime est 4 500–7 000× plus légère par agent dormant que Docker+Python en termes de RAM. Validé par T6-qualif K=3 (H-densité → partiellement validé, 2026-05-22).

**Pari 4 — Supervision asymétrique, pas supervision en temps réel (ADR-0006)**  
Le superviseur humain observe (log causal), intervient (rollback, révocation), autorise. Il n'interagit pas en temps réel. Ce pari dimensionne la granularité du log causal et la latence de P3.

**Pari 5 — Capabilities révocables, pas identité Unix (ADR-0005)**  
Un agent ne détient de droits que via des capabilities explicites, déléguées par son parent, révocables à tout moment. La révocation est lazy avec propagation en chaîne. Validé fonctionnellement (S4, SEF-3/S9).

**Pari 6 — Agents auto-déclarants leur profil watchdog (ADR-0025)**  
L'agent déclare son `AgentProfile` (`Algo`, `LlmShort`, `LlmLong`, `Batch`) au spawn. Le watchdog est un garde-fou contre les boucles infinies, pas une frontière de sécurité.

---

## Ce qui a été démontré

### Phases 1–4 — Lab Python/Docker

Mémoire sémantique, causalité concurrente, rollback, capabilities et révocation — en Python/Docker via un daemon HTTP. Référence historique ; plus maintenu. Leçons L1–L45 dans `lab/LESSONS.md`.

### Phase 5 — Layer 0 RocksDB

Substrat Rust/Wasmtime/RocksDB. Modules `causal-log`, `store`, `capabilities`, `runtime` validés en isolation (33 tests). Benchmark T5 : p99 371–502 µs sur 10⁸ entrées (NVMe AWS, 2026-05-15).

### PoC E2E — Agent LLM réel sur Wasmtime (2026-05-16)

53 tests verts, 4/4 scénarios pass. ABI `agent_infer`, composition cross-agent (S1), self-rollback (S2), pool d'inférence borné (S3), rollback scheduler + révocation (S4).

### Phase 6 — Propriétés fortes C1 (2026-05-17)

D9 (profils watchdog `AgentProfile`), D-Q-V2.2 (journal de compensation `0x11/0x12`), D-Q-V2.6 (`InferenceQueue` bornée avec priorité, `NoSlot (3)`). ADR-0022–0025.

### Phase 7 — Qualification + Scheduler unifié + SEF (2026-05-18 → 2026-05-26)

- T5-qualif K=3 : P3a → validé (p99 pire cas 4 855 µs, ADR-0026). T5-bis-thermal (ADR-0032) : variance p99 causée par compaction RocksDB L0, pas thermique. T6-qualif K=9 : ratio 4 500–7 375×, H-densité → partiellement validé.
- Scheduler unifié C1+C2 : `InferenceQueue` + `IoAdmissionQueue` + pipeline C2→C1, réveil à la demande (ADR-0030/0031). Scénarios S5–S12 + S13 (persistance) + S14 (P3a).
- SEF-1→SEF-6 complets : P6 atomicité crash (S6/SEF-4), P2 rollback (S7/SEF-2), P5 déterminisme (S8/SEF-6), P4 isolation (S9/SEF-3).

### Phases 8–10 — PoC seL4 (2026-05-27 → 2026-05-30)

Stack runtime seL4 (Wasmtime `min-platform` no_std, ADR-0037). Store natif (redb fork no_std sur virtio-blk, ADR-0038/0042). Jalons C.1→C.11-prov (AArch64 QEMU) : hello world, root task custom, Wasmtime intégré, driver block virtio, redb/virtio-blk, intégration 2-processus, N agents, store persistant, réouverture, W^X JIT, module WASM non confié. ADR-0037–0049. PoC clos (ADR-0049).

### Mise à l'épreuve adversariale (ADR-0050, 2026-05-30)

Campagne attaque (vs validation). Gate soundness SEF-8 : 5 INSTANCIÉE, 5 PROXY, 1 SUR-GARANTIE sur P1–P6 + SEF-7. Axe 1 SEF-9 : confused-deputy rate-limit ↔ audit — finding confirmé (1b ÉCHOUE, 1a INTACTE). Axe 3 SEF-10 : fenêtre de référence pendante cross-store démontrée ; durabilité power-loss différée (mur infrastructure). Correctifs ADR-0051.

---

## Périmètre des revendications

### Ce qui est démontré

- **Cœur logique P2/P4/P5/P6** : rollback transactionnel (P2, SEF-2 PASS), isolation capabilities (P4, SEF-3/SEF-9), déterminisme de transition (P5, SEF-6), atomicité crash-processus (P6, SEF-4/SEF-10 correctifs). Validé sur Linux PoC (phases 5–10). Mis à l'épreuve adversariale (SEF-8/9/10, campagne P2/P3/P5, ADR-0053).
- **P3a traçabilité causale ≤10 ms** : SEF-5 PASS, p99 1,4–1,9 ms sur 10⁸ actions en lecture seule (substrat Linux/NVMe). Fonctionnel seL4 (C.8+, redb/virtio-blk) — latence absolue non mesurée sur QEMU (non recevable, ADR-0046).
- **Densité Wasmtime vs Docker+Python** : T6-qualif ×4 500–7 375× (H-densité partiellement validé, substrat Linux).
- **Stack seL4 AArch64 opérationnelle** : arc C.1→C.11-prov sur QEMU virt AArch64. Isolation 2-processus, P6/P6-N en régime crash-processus, I4 (non-interférence Biba), W^X JIT, isolation WASM non confié, provenance canal non-trusted.

### Ce qui est hors scope et pourquoi

- **Comparaison quantitative P1 vs Linux+containers** (Critère 1 strict) : mesurée qualitativement (Wasmtime vs Docker+Python ×5 PASS). La qualification sur hardware serveur a été abandonnée parce que les résultats Linux/NVMe ne prédisent pas les performances seL4-natif (décision architect 2026-05-27, ADR-0049 §D3). Ce n'est pas un benchmark manquant — c'est une décision de ne pas qualifier ce qui ne se transfère pas.
- **D-P3a** (P3a sur seL4 hardware réel) : jalon créé (`poc/sel4-hello/d-p3a/`) mais non exécuté — bloqué infrastructure (board réelle ou NVMe passthrough). QEMU n'est pas un substrat recevable pour la mesure de latence de stockage.
- **Power-loss / β** : hors scope par décision explicite (ADR-0045 Q2=α). Exige le même substrat que D-P3a.
- **Séparation CAS-autoritaire / index-reconstructible** (ADR-0038 §3) : cible architecturale spécifiée, non instanciée. Le store réel est un store redb transactionnel monolithique ; P6 est tenue par sur-garantie (ADR-0049 §D2, L82).

L'abandon de la qualification hardware Linux et le report de D-P3a découlent d'une décision de cohérence : valider sur un substrat différent de la cible ne fournit pas d'information utile. Ce périmètre est un résultat de la démarche, pas une limitation non assumée.

---

## Ce qui reste ouvert

### Corrections spec/02 — soldées (Phase 11 T2, ADR-0051)

Le gate SEF-8 avait surfacé quatre points de sur-revendication. Tous ont été traités :
- §P2 : « O(log N) » → O(depth) corrigé, borne temporelle tenue.
- §P4 : audit qualifié « jusqu'au rate-limit » + correctif #6 (agrégation par resource).
- §P6 : asymétrie orphelin toléré / référence pendante inscrite ; dette cross-store tracée (#7b).
- §P5 : spec déjà correcte — la garantie est conditionnelle à S6 (agent déterministe). Dette d'oracle (#3) dormante.

### Déclencheurs dormants seL4 (ADR-0049 §D3)

- **D-P3a** : latence P3a sur média réel sous seL4 (board réelle ou NVMe passthrough — QEMU non recevable).
- **Power-loss / β** : substrat média réel requis (identique à D-P3a).
- **C.12+** (setjmp réel, watchdog temporel, signature) : déclenchés par ≥ 2 agents par VSpace / SLA mur / réseau-PKI.
- **GC orphelins redb** : déclenché par croissance non bornée observée sur cycles reopen.

### Questions ouvertes de spec

- **ADR-0015** : propagation d'erreur cross-agent (réservé — déclenché par le premier incident réel).
- **ADR-0016** : escalade typée (réservé — déclenché si filtrage par `verdict == Timeout` insuffisant).
- **C3** : épuisement épistémique (mémoire sémantique des sessions) — hors scope actuel.

---

## Structure du dépôt

```
QUICKSTART.md      Comment faire tourner le PoC
spec/              Design document (10 fichiers, P1–P6, plafonds C1–C3)
decisions/         ADR-0001–0053 + b3-storage-research.md
agents/sel4.md     Référence technique seL4/rust-sel4 (citée par poc/sel4-hello)
poc/               PoC Linux (Rust/Wasmtime/RocksDB, mesuré) + stack seL4 cible (Wasmtime no_std/redb)
  README.md        Doc technique du PoC (modules, ABI, scénarios)
  scenarios/       S1–S14 + SEF-8/9/10 + harness run-all.sh
  agent-sdk/       Crate Rust→WASM
  sel4-hello/      PoC seL4 AArch64 (C.1→C.11-prov)
  redb-fork/       Fork redb no_std (C.5-A)
  redb-p3a/        Benchmark P3a redb Linux
lab/               Implémentation Python/Docker (phases 1–4 — référence historique)
  LESSONS.md       Leçons empiriques cumulatives (L1–L89+)
benchmarks/        Spécifications workloads W1–W3, protocoles T5/T6, harnesses
results/           Synthèses T5/T5-bis/T5-ter/T6
docs/              Documentation transverse
  guides/          Guides d'apprentissage FR/EN (+ PDF)
  demo/            Démo TUI : guide, script, échantillons de code
  archive/         Briefs de chantier figés (poc_E2E.md, phase6.md)
references/        Notes de lecture, bibliographie
```

## Où trouver quoi

| Besoin | Fichier |
|---|---|
| Faire tourner le PoC | `QUICKSTART.md` |
| Doc technique du PoC | `poc/README.md` |
| Design formel (vision, propriétés, hypothèses) | `spec/` |
| Chaîne de décisions architecturales | `decisions/INDEX.md` |
| Dettes actives + déclencheurs dormants | `TODO.md` |
| Leçons empiriques | `lab/LESSONS.md` |
| Briefs de chantier (archives) | `docs/archive/poc_E2E.md`, `docs/archive/phase6.md` |
| Benchmarks workload + protocole | `benchmarks/` |
| Résultats T5/T6 | `results/` |

## Statut

| Phase | Contenu | État |
|-------|---------|------|
| 1–4 | Lab Python/Docker : mémoire, causalité, rollback, capabilities | ✅ |
| 5 | Layer 0 RocksDB, benchmarks T5 | ✅ |
| PoC E2E | Agent LLM réel sur Wasmtime : S1–S4, agent_infer, InferencePool | ✅ |
| 6 | Propriétés fortes C1 : D9, D-Q-V2.2, D-Q-V2.6, S5 | ✅ |
| 7 | Qualification T5/T6, scheduler unifié C1+C2, SEF-1→6, S5–S14 | ✅ |
| 8–10 | PoC seL4 AArch64 : C.1→C.11-prov, ADR-0037–0049 | ✅ |
| Mise à l'épreuve | Campagne adversariale ADR-0050 : SEF-8/9/10, correctifs ADR-0051 | ✅ (durabilité power-loss différée, harness I-CSR construit) |
| 10 (inférence) | Scheduler d'inférence sous OllamaBackend : P10-S3/S5 PASS (ADR-0052) | ✅ (verdicts non transférables hardware GPU cible) |
| 11 | Remontée spec : spec/09 consolidé, spec/02 et spec/08 amendés (T1–T4, ADR-0049 §D4) | ✅ |
| 12 | Campagne adversariale P2/P3/P5 : SEF-12 PASS, SEF-13 PASS, A-P5 clos (ADR-0053) | ✅ |

---

## Licence

Ce dépôt mélange du code et de la documentation, sous deux licences :

- **Code** — tout `poc/` (Rust, WASM, scripts) : **Apache-2.0** (`LICENSE`). Réutilisation libre, y compris commerciale, avec attribution ; inclut une concession de brevet explicite.
- **Documentation et écrits** — `spec/`, `decisions/`, `lab/`, `docs/`, `benchmarks/`, `results/`, `red-team/`, `references/`, `agents/sel4.md`, `TODO.md` et les `*.md` à la racine : **Creative Commons CC-BY-4.0** (`LICENSE-DOCS`). Reproduction et adaptation libres avec attribution.

SPDX : `Apache-2.0` (code) / `CC-BY-4.0` (docs).

© 2026 Joey Leonard. *(Adapter ce nom d'attribution si besoin avant publication.)*
