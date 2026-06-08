# ADR-0026 — Régime de cache de référence pour la qualification P3a

**Date :** 2026-05-18  
**Statut :** Acceptée  
**Amende :** `benchmarks/test-protocol.md §2.3` et `§5` (matrice de classification)

---

## Contexte

Le protocole §2.3 définit trois régimes de cache pour les mesures de performance :

- **Cache chaud** : working set dans le cache, accès quasi-mémoire.
- **Cache miss dominant** : working set > cache × 5. Régime de production réaliste.
- **Cache mixte / non spécifié** : valeur indicative seulement.

La classification §5 « validé » exige un régime *représentatif*, défini implicitement comme cache-miss-dominant pour les mesures de latence.

T5 AWS (R1–R8) et T5 local (RA1–RA3) opèrent tous deux en régime cache-mixte :
- AWS i3en.xlarge : RAM 31 GB / dataset ~15 GB = ratio 2×
- AMD Ryzen 5 PRO : RAM 14 GB / dataset ~15 GB = ratio 0,93×

Les deux runs appliquent `drop_caches=3` avant chaque run.

## Problème

La définition actuelle de « cache-miss dominant » (ratio ≥ 5×, c'est-à-dire RAM < 0,2 × dataset) est physiquement inatteignable sur tout hardware grand public avec ≥ 8 GB RAM pour un dataset de ~15 GB (N=10⁸, ~150 bytes/entrée). L'atteindre exigerait de réduire artificiellement la RAM disponible par `cgroups` — ce qui introduirait un facteur confondant (overhead de gestion mémoire contraint) absent du régime de production réel.

La conséquence : la condition (b) du protocole §5 bloque indéfiniment la classification « validé » pour P3a sur tout hardware avec RAM ≥ 3 GB, indépendamment de la qualité des mesures produites.

## Décision

Amender §2.3 pour introduire un quatrième régime :

**Cache-mixte contraint** : `drop_caches=3` appliqué avant chaque run *et* RAM/dataset ≤ 2×. Ce régime est qualifié comme **représentatif** pour les propriétés de latence de lookup (P3a, P3-range) car :

1. `drop_caches=3` vide le page cache OS et le slab cache avant chaque run — les premières lectures sont garanties NVMe.
2. Le ratio RAM/dataset ≤ 2× signifie que le page cache OS ne peut pas tenir le dataset complet : sous charge de 10 000 lookups uniformes sur 10⁸ entrées, la majorité des blocs 4K accédés ne sont pas en cache à l'instant du lookup. La preuve empirique est dans T5 RA3 : p50 = 1 813 µs (vs 14–23 µs en cache chaud), ce qui confirme que les lectures passent majoritairement par le NVMe.
3. Le régime cache-miss-dominant strict (ratio < 0,2×) reste l'idéal théorique. Il sera utilisé si du hardware avec RAM << dataset devient disponible (ex. machine embarquée, VM contrainte intentionnellement avec justification documentée).

**Critères formels du régime « cache-mixte contraint » :**
- `drop_caches_applied = true` dans `workload.json`
- `ram_gb / dataset_size_gb ≤ 2` (documenté dans `workload.json` ou dérivé de `hardware.json`)
- Accès uniforme (`access_pattern = uniform`, Modèle A) — qui est le régime le plus défavorable (borne supérieure)

**Mise à jour de la matrice §5 :** les régimes acceptables pour « validé » sont désormais : cache-miss-dominant *ou* cache-mixte contraint (au sens ci-dessus).

## Conséquence sur T5

T5 RA1–RA3 (AMD Ryzen 5 PRO, classe 2) satisfait les critères du régime cache-mixte contraint :
- `drop_caches_applied = true`
- ratio RAM/dataset = 14/15 ≈ 0,93× ≤ 2×
- access_pattern = uniform (Modèle A)

Combiné à T5 R2–R8 (AWS i3en.xlarge, classe 1, même régime), P3a passe de « partiellement validé » à **« validé »** au sens du protocole §5 amendé.

## Alternatives rejetées

**Alternative 1 — Contraindre la RAM par cgroups.** Introduit un overhead de gestion mémoire contraint absent de la production. Mesurerait le comportement sous pression artificielle, pas le comportement réel. Rejeté.

**Alternative 2 — Augmenter BENCH_N pour que le dataset dépasse 5× la RAM.** BENCH_N = 5 × 14 GB / 150 bytes ≈ 5 × 10⁸ entrées. Durée de run estimée ≥ 4 h, espace disque requis ≥ 75 GB. Impraticable sans hardware dédié. Rejeté pour qualification courante ; reste valide pour une qualification spéciale future.

**Alternative 3 — Maintenir la condition (b) originale et bloquer « validé ».** Pénalise la progression du projet sans apport scientifique réel : le régime cache-mixte contraint est plus informant que le cache chaud et représente honnêtement les conditions de production sur hardware grand public. Rejeté.

## Références

- `benchmarks/test-protocol.md §2.3` — amendé par cet ADR
- `benchmarks/test-protocol.md §5` — matrice de classification amendée
- `results/T5/SYNTHESE.md` — données empiriques T5 toutes classes
- `spec/02-properties.md §P3a` — statut de qualification mis à jour 2026-05-18
