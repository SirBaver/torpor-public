# Reproduction autoritaire — blog-08 (le vrai système)

**Substrat de mesure :** PoC Linux — Rust / Wasmtime / **RocksDB** / NVMe consumer (R-blog-1).
**Régime :** R1 (contrôle des effets) — actif quel que soit l'endroit où tourne l'inférence.
**Statut :** la scène `lineage` = **même moteur** que `effects`, payloads data engineering. Elle illustre, ne mesure pas une capacité nouvelle. Mono-tenant (ADR-0036).

```bash
git clone <url-os-public> && cd os-public/poc
git checkout blog-08-demo            # permalink épinglé, jamais main

# 0) Prérequis : compiler les agents WASM de la scène (depuis poc/)
CXXFLAGS="-include cstdint" cargo build --target wasm32-unknown-unknown \
  -p agent-sdk --release --example data_profiler --example data_transformer \
  --example data_accessor

# 1) Démo interactive : pipeline profileur → transformateur
CXXFLAGS="-include cstdint" cargo run -p os-poc-runtime --features demo-tui \
  --bin demo-tui -- --scene lineage
#   [m] MODÉLISATION : arbre lineage lisible (source → profil → artefact → publication)
#   [t] FALSIFIER    : altère 1 octet amont → P3 « Violation détectée », id stocké ≠ recalculé
#   [r] ROLLBACK     : ramène le transformateur à son snapshot → la cap de publier est RÉVOQUÉE
#   [p] PUBLIER      : après rollback → DENIED (0x14) tant que pas re-dérivé proprement
#   [g] RE-DÉRIVER   : nouvelle branche propre, re-publication sous cap fraîche
#   [x] INTRUS       : agent hors périmètre → accès REFUSÉ et tracé (0x14)

# 2) Auto-test headless (aucun terminal interactif requis) : 12 assertions
CXXFLAGS="-include cstdint" cargo run -p os-poc-runtime --features demo-tui \
  --bin demo-tui -- --selftest-lineage
# attendu : 12× [PASS] puis « SELFTEST lineage : PASS » (exit 0)
```

**Variantes d'inférence.** En rejeu (défaut), les réponses du modèle sont en conserve,
ce qui isole le contrôle des effets de toute performance d'inférence (garde-fou F1).

```bash
# Modèle local réel (Ollama) au lieu du rejeu
... --bin demo-tui -- --scene lineage --live

# Modèle faillible : profil FAUX, mais lineage INTÈGRE (article §27)
... --bin demo-tui -- --scene lineage --llm-wrong
```

**Vérification d'intégrité hors interface** (le DAG de la scène est un journal causal standard) :

```bash
CXXFLAGS="-include cstdint" cargo run -p os-poc-runtime --bin log-verify -- <chemin-log>
# exit 0 intègre · 1 altération (désigne l'action_id fautif) · 2 erreur
```

**Limites.** Régime R1 uniquement (pas de mesure de densité ici, cf. blog-04). Le lineage
est **borné à CE runtime** : il ne suit pas la donnée une fois sortie du système (provenance
structurelle, pas métadonnée déclarative type OpenLineage). Mono-tenant.

Transcript réel attendu : voir `expected/` (capturé au tag). Cas d'usage data engineer :
`docs/demo/atelier-data-inge-use-case.md` et `docs/demo/demo-tui-guide.md`.
