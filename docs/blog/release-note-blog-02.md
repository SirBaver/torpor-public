<!--
GABARIT DE RELEASE NOTE — à publier comme corps de la GitHub Release `blog-02-dag-causal`
sur os-public (FR) ET os-public-en (EN, slug issu du termbase GATE).
Les champs ⟨à épingler⟩ sont à figer au moment du tag : remplacer par le permalink
réel pointant sur le tag (jamais sur `main`) et les numéros de ligne exacts.
Invariants non négociables : (a) substrat nommé sur la même ligne que la borne ;
(b) moteur nommé (RocksDB=PoC Linux / redb=cible seL4) ; (c) permalink épinglé au tag.
-->

# blog-02 — La flèche entre deux décisions EST un hash

**Régime :** R1 (effets) · **Statut :** prouvé · **Substrat de mesure :** Linux / **RocksDB** / NVMe consumer (Ryzen 5 PRO 4650U + WD SN530) — R-blog-1

## Claim → Preuve

| Claim de l'article | Borne | Preuve (permalink épinglé `@blog-02-dag-causal`) | Substrat |
|---|---|---|---|
| Lookup causal par identifiant, p99 | 23 µs @ 10⁶ entrées, 4 écrivains | ⟨results/.../T5-p3c/verdict.json:Lxx⟩ | Linux/RocksDB |
| Lookup causal, lecture seule à l'échelle | ~1,4–1,9 ms @ 10⁸ entrées | ⟨results/.../SEF-5/verdict.json:Lxx⟩ | Linux/RocksDB |
| Historique tamper-evident (clé = SHA256 du contenu) | propriété (pas une borne) | ⟨poc/causal-log/src/...⟩ · `integrity::verify_content_addressing` | substrat Linux |
| DAG `caused_by[]`, pas arbre (pari n°1) | propriété | ⟨decisions/0003-modele-causal-dag.md⟩ | — |

## Reproduire

```bash
git clone <url-os-public> && cd os-public/poc
git checkout blog-02-dag-causal
# Démo interactive : couche preuve [d] + falsification [t] (stored ≠ recalc, juge orphelin)
CXXFLAGS="-include cstdint" cargo run -p os-poc-runtime --features demo-tui \
  --bin demo-tui -- --scene effects
# Vérificateur tiers (exit 0 intègre / 1 mismatch / 2 erreur)
CXXFLAGS="-include cstdint" cargo run -p os-poc-runtime --bin log-verify -- <chemin-log>
# attendu : sur un journal corrompu d'une entrée → exit 1, pointe la bonne action_id
```

## Limites (R-blog-3 / O6 / frontière LLM)

- **Infalsifiable = non-réécriture détectable**, PAS « le système est imprenable » ni « l'agent décide juste ». Propriété de traçabilité, pas label de sûreté.
- **Borne conditionnelle par construction** : 23 µs caractérise le substrat Linux/**RocksDB**. La cible seL4 utilise **redb** — latence non transférée, aucune mesure seL4-native planifiée (ADR-0065).
- Les opérations de preuve/falsification/vérification agissent sur le **journal**, pas sur l'inférence (F1) : identiques en rejeu et en `--live`.

<!--
Release jumelle EN (`os-public-en`, même tag `blog-02-dag-causal`) : mêmes claims,
permalinks vers les chemins EN, slug et termes ancrés sur TERMBASE.md (GATE).
-->
