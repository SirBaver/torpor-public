# Avis externe — T5 AWS i3en.xlarge & préconditions Q1–Q3, C2

**Date :** 2026-05-15
**Sujet :** revue du run T5-qualif (4 runs, AWS i3en.xlarge, 2026-05-15) et recommandations sur les quatre préconditions ouvertes au TODO.
**Statut :** brouillon de revue externe, à intégrer ou à contester avant la prochaine étape.

---

## 1. Analyse factuelle du run T5

### 1.1 Ce qui tient

Trois éléments du run sont solides au sens du protocole et méritent d'être nommés explicitement dans la capitalisation (lesson L44 à écrire).

**Le p99.9 reste discipliné.** Sur les quatre runs, le p99.9 est dans 572–684 µs, soit 1,2–1,4× le p99. Pour un benchmark RocksDB en régime cache-miss avec dataset >> block cache, c'est une queue de latence remarquablement courte. On voit fréquemment des ratios p99.9/p99 de 5–10× quand la compaction frappe pendant la fenêtre de mesure. L'absence de cet effet ici suggère que la DB était stabilisée (compaction terminée avant le bench) et que les 10 000 mesures ne sont pas tombées sur un stall.

**L'instance était propre.** `cpu_steal ≤ 0,05 %` sur tous les runs, `io_wait` cohérent à 10,8–12,1 % entre runs. Pas de neighbor agressif, pas de variabilité virtualisation qui contaminerait les mesures.

**La prédiction L19 est tombée juste.** L19 prédisait « p99 dans la fourchette 100–500 µs » pour le régime cache-miss à N=10⁸. Observé : 371–502 µs. La calibration mentale du projet sur le coût d'un cache miss NVMe est bonne — utile à noter pour les futures prédictions.

**F2 du L43 ne se réalise pas.** L43 craignait une dégradation de la marge de « ~4 ordres de grandeur » entre cache chaud (×900) et cache miss. Observé : ×900 → ×20, soit 1,7 ordre. La fondation H-causal-latence tient nettement mieux que le pire scénario envisagé.

### 1.2 Ce qui ne tient pas — trois trous procéduraux

**Trou n° 1 — Le régime de cache déclaré n'est pas le régime mesuré.**

L'instance dispose de 31 GB de RAM. Le dataset T5 fait ~15 GB. Le block cache RocksDB est plafonné à 256 MB par ADR-0011. La RAM disponible pour le page cache OS, après OS et processus, est de l'ordre de 28–29 GB — soit ~2× la taille du dataset.

Le protocole §2.3 définit `cache-miss-dominant` comme « working set excède le cache d'un facteur ≥ 5× » et précise que le cache inclut explicitement le page cache OS. Avec un ratio dataset/cache de 0,5×, on est en `cache-mixte` au sens strict du protocole, qui stipule lui-même que cette catégorie est « valeur indicative seulement, à ne pas comparer à une cible quantitative ».

L'évolution R1 (p99=502 µs) → R4 (p99=371 µs) confirme cette lecture : le page cache OS s'accumule entre runs, et R4 mesure une situation plus chaude que R1. Le p50 stable à 14–17 µs sur tous les runs est cohérent avec des hits RAM, pas des cache misses au sens disque.

**Conséquence :** la marge ×20 reportée est *optimiste*. La valeur la moins contaminée est probablement R1 (502 µs, ×15), et un vrai régime cache-miss strict (page cache vidé) donnerait sans doute p99 dans la fourchette 800–1 200 µs (×8–12).

**Trou n° 2 — `git_commit: null` viole le §3.2 du protocole.**

Le protocole §3.2 dit explicitement : « git_dirty: true place en indicatif. Une mesure publiable comme validée doit être faite sur un working tree propre, sur un commit identifiable. » L'absence pure et simple de `git_commit` est plus problématique que `git_dirty: true` — il n'y a aucune façon, à partir de `software.json`, de retrouver le code source exécuté. Le commit `38c4324` mentionné en local n'est pas attesté côté instance.

Strictement par le protocole, cette violation pourrait dégrader la classification de « partiellement validé » vers « indicatif ». À toi d'arbitrer.

**Trou n° 3 — Le débit séquentiel NVMe est sous-mesuré.**

`fio` mesure 768 MB/s en lecture séquentielle. AWS spécifie pour i3en.xlarge une bande passante NVMe instance store de l'ordre de 2,5–3 GB/s séquentiel et ~525 000 IOPS aléatoires 4K. L'écart suggère que la mesure a été faite avec `iodepth=1, numjobs=1`, ce qui caractérise le pire cas mono-thread plutôt que la classe de hardware.

Cette mesure ne contamine pas T5 directement — la charge T5 est en lookups aléatoires, et les latences observées (p99 ≈ 500 µs) sont cohérentes avec un bon NVMe en accès random. Mais elle alimente le dimensionnement C2 actuel (15 agents/s), qui est donc fondé sur la borne basse mono-thread plutôt que sur la capacité réelle du drive.

### 1.3 K effectif

R1 a été marqué comme ayant deux bugs (fio + `date +%s%3N` sur kernel 7). Tu le marques en footnote, mais en pratique R1 est non conforme au manifest. K effectif descend à 3 (R2/R3/R4) — le seuil minimum exact du protocole §5 pour « partiellement validé ». Aucune marge en cas de découverte d'un autre artefact à la relecture.

---

## 2. Recommandations sur le run T5 lui-même

Avant de payer pour une seconde instance type pour viser « validé », trois fixes sont à appliquer. Coût combiné : un après-midi de travail + un re-run de ~30 minutes.

**Fix 1 — `drop_caches` dans le harness.** Ajouter avant chaque mesure de batch :

```bash
sync && echo 3 | sudo tee /proc/sys/vm/drop_caches > /dev/null
```

Ça remet R2/R3/R4 dans un état comparable à R1 et rend le régime déclaré cohérent avec le régime mesuré. Le bench reste plus rapide qu'avec dataset >> RAM, mais au moins le `workload.json` ne ment plus.

**Fix 2 — Traçabilité du code source.** Deux options non exclusives :

L'option propre : utiliser `git archive 38c4324 -o source.tar` côté local, transférer le tarball, et inclure le SHA du commit dans le tarball (que `git archive` y dépose automatiquement). Reproductibilité bit-pour-bit.

L'option pragmatique : calculer un `sha256` récursif de l'arbre source (par exemple `find . -type f -not -path './target/*' | sort | xargs sha256sum | sha256sum`) côté local *et* côté instance, et le mettre dans `software.json` comme `source_tree_sha256`. Vérifie que l'arbre exécuté est exactement celui que tu prétends, sans dépendre de `.git/`.

**Fix 3 — Requalifier le débit NVMe.** Une seule commande supplémentaire dans le manifest :

```bash
fio --name=seq_read --rw=read --bs=128k --iodepth=32 --numjobs=4 \
    --direct=1 --filename=/dev/nvme1n1 --runtime=30 --time_based
```

Reporter dans `hardware.json` les deux mesures : `storage_seq_read_mb_s_qd1` (mono-thread QD=1) et `storage_seq_read_mb_s_qd32` (multi-thread haut QD). C'est la première qui caractérise le coût d'une opération unitaire, la seconde qui caractérise la classe hardware.

**Re-run minimal :** un seul run propre (drop_caches + manifest correct) après ces fixes, à comparer aux 4 existants. Si le p99 grimpe à 800–1 200 µs comme anticipé, c'est une donnée plus honnête à publier, et la marge tombe à ×8–12 — toujours conforme à la cible 10 ms, mais sans illusion.

---

## 3. Q1 — Portée de la borne P3

### 3.1 Le fond du problème

La question pose trois portées candidates : (a) lookup seul `get(action_id)`, (b) end-to-end `emit→fsync→lookup`, (c) tail-bounded sous contention multi-agent. Le bench T5 actuel mesure (a) sur DB statique et en lecture seule. C'est la portée la plus étroite des trois.

Le problème de fond : (a) est ce qui est facile à mesurer, mais ce n'est pas ce qui dimensionne la latence perçue par un agent en production. Un agent qui émet une action et veut immédiatement la relire — pour vérifier qu'elle est durable, pour la chaîner causalement, etc. — paie au minimum (b). Et un système qui sert N agents concurrents paie (c).

### 3.2 Recommandation : trois sous-propriétés, pas une

Plutôt qu'arbitrer en faveur de l'une des trois portées, je recommande de décomposer P3 explicitement en P3a, P3b, P3c, chacune avec sa propre cible, sa propre méthode de mesure, et sa propre classification :

| Sous-propriété | Portée | Cible suggérée | Mesure |
|---|---|---|---|
| **P3a** | `get(action_id)` sur DB statique | p99 ≤ 10 ms | T5 actuel — déjà partiellement validé |
| **P3b** | `emit → fsync WAL → get` end-to-end | p99 ≤ 20 ms (WAL fsync 1–5 ms + lookup) | À implémenter, T5-bis |
| **P3c** | p99 sous N agents concurrents écrivant + lisant | p99 ≤ 50 ms à N=8, p99 ≤ 100 ms à N=32 | À implémenter, T5-ter — différé |

La cible de P3b doit accommoder le coût WAL : sur NVMe rapide, `fsync` typique 0,5–3 ms ; sur SSD SATA, 5–20 ms. La borne 20 ms laisse une marge raisonnable et reste utile (la perception d'un humain pour « instantané » est ~100 ms).

P3c demande un benchmark multi-tenant qui n'existe pas, et qui dépend de Q2 (modèle de working set). Le marquer comme **réservé** avec un trigger explicite (« déclenché quand T5-bis passe et qu'on a un workload de référence multi-agent ») suit le pattern ADR-0015/0016.

### 3.3 Conséquence pour T5 existant

Le run actuel mesure P3a et reste pleinement valable pour cette sous-propriété. Pas de rétrofit nécessaire. Le `verdict.json` doit juste être amendé pour préciser « P3a partiellement validé », pas « P3 partiellement validé ».

### 3.4 Conséquence pour ADR / spec

Mettre à jour `spec/02-properties.md §P3` avec la décomposition. Marquer P3a comme propriété mesurée, P3b comme propriété à mesurer (avec critère de déclenchement de T5-bis), P3c comme réservée. Ne pas réécrire l'ADR-0002 — il choisit RocksDB pour des raisons architecturales qui ne dépendent pas du choix entre P3a/b/c.

---

## 4. Q2 — Modèle de working set par agent

### 4.1 Le fond du problème

Sans modèle, on ne peut pas dimensionner le block cache (C2), on ne peut pas écrire T5-multi-tenant (P3c), et on ne peut pas calibrer BlobDB (Q3). Mais on n'a pas de données de production. Le piège : choisir un modèle silencieusement parce qu'il « semble raisonnable », sans le rendre attaquable.

### 4.2 Recommandation : deux modèles déclarés, mesurer les deux

Plutôt que de choisir un modèle unique, en déclarer deux explicitement dans `reference-workload.md §W1`, et exécuter T5 avec les deux patterns d'accès. La vraie p99 production sera entre les deux ; aucune des deux ne ment toute seule.

**Modèle A — pessimiste (« no-locality »).** Chaque agent peut lire n'importe quelle entrée de son historique causal avec probabilité uniforme. Hot set par agent = historique causal complet. Recouvrement inter-agent = 0 % (isolation forte). C'est ce que mesure `populate_synthetic` aujourd'hui avec ses lookups uniformes — la borne supérieure du coût.

**Modèle B — réaliste (« recency-biased »).** Distribution Zipf ou exponentielle sur l'âge causal : la majorité des lookups portent sur les K dernières actions (K ≈ 50–200), une queue lourde sur l'historique profond. Recouvrement inter-agent ≈ 5–15 % via références causales (parent→enfant, merge, send_caused_by). C'est plus proche de l'usage anticipé d'un agent LLM (chaînage causal immédiat, mémoire de travail à horizon court).

Le modèle B n'est pas observé, c'est une convention. Mais c'est une convention argumentable, basée sur (i) la nature du chaînage causal qui privilégie les parents immédiats, (ii) la borne de session à 10K actions (ADR-0012) qui limite mécaniquement la profondeur d'accès courant, (iii) la littérature agent LLM (LangChain, CAMEL, AutoGPT) qui pointe systématiquement vers une mémoire de travail courte.

### 4.3 Conséquence pour le bench

Modifier `populate_synthetic` pour exposer un paramètre `--access-pattern=uniform|recency` (ou deux benchs distincts). Reporter p50/p95/p99/p99.9 sous chaque pattern. La différence entre les deux donne une mesure de la sensibilité du système à la locality d'accès — donnée utile en soi, indépendamment de la « vraie » distribution future.

### 4.4 Conséquence pour Q3 et C2

Modèle A → C2 dimensionné sur worst case cache-miss → 15 agents/s (chiffre actuel honnête).
Modèle B → C2 dimensionné sur cache-friendly → probablement 50–100 agents/s.

Publier les deux dans `spec/07-plafonds-architecturaux.md` plutôt qu'un seul chiffre. Le scheduler Phase 3 prendra une décision en run-time fondée sur la mesure réelle de hit ratio, pas sur une estimation a priori.

---

## 5. Q3 — `emit_payload_size_distribution`

### 5.1 Le fond du problème

Q3 a une contrainte structurelle que la formulation actuelle masque : `populate_synthetic` génère des entrées de taille fixe (~100 bytes par L18). Le bench T5 est *structurellement incapable* de produire un histogramme `emit_payload_size_distribution` informatif — la distribution est dégénérée par construction. L'exigence du protocole de remonter cet histogramme depuis T5 est donc vide.

Deux voies, pas trois.

### 5.2 Recommandation : adopter une distribution de convention, documentée

L'option « attendre un workload réel » bloque indéfiniment ADR-0017 (BlobDB) sur une donnée qui n'arrivera pas avant Phase 3. L'option « laisser `null` indéfiniment » alimente la dette technique. La voie pragmatique : adopter une distribution de convention, la documenter dans `reference-workload.md`, et la défendre comme telle.

**Distribution de convention proposée**, basée sur les types d'`EmitType` d'ADR-0010 et l'observation des payloads typiques d'agents LLM :

| Type d'action | Taille typique | Source de l'estimation |
|---|---|---|
| `Spawned`, `Active`, `Suspended`, `Terminated` | 64–128 B | métadonnées d'état, formats fixes |
| `Introspect` | 73 B | format binaire fixe documenté (L29) |
| `SelfRollback`, `SchedulerRollback` | 16–32 B | distance + target_seq + flags |
| `ValidationRequest`/`Response` | 32–64 B | risk_level + verdict |
| `SessionBoundary` | 256–1 024 B | résumé causal court |
| Tool call args (custom) | 256–2 048 B | typique LLM function-call JSON |
| Memory write payload | 512–4 096 B | observation, fait extrait |
| Web/file observation | 4 KB–64 KB | contenu indexé partiel |

Distribution composite résultante (à pondérer selon le mix de types attendu) :

```
p50  ≈    256 B
p90  ≈  4 096 B
p95  ≈  8 192 B
p99  ≈ 32 768 B
max  ≈ 65 536 B
```

### 5.3 Conséquence pour BlobDB et ADR-0017

Le seuil `min_blob_size` pour BlobDB doit se placer au point où l'amortissement compense l'overhead d'indirection. Sur la distribution ci-dessus, le p90 (4 KB) est le candidat naturel : 10 % des entrées sortent via BlobDB, 90 % restent inline en SST, et le seuil est en pleine queue de distribution (pas dans un mode dense où une petite variation déplace beaucoup d'entrées).

Recommandation concrète pour ADR-0017 : déclarer `min_blob_size = 4 KB` comme valeur initiale, marquer la décision comme **provisoire** avec clause de réévaluation après le premier workload W2 réel mesuré. C'est analogue à la formulation « Acceptée (provisoire) » d'ADR-0011.

### 5.4 Conséquence pour T5

Si tu veux que T5 informe la calibration BlobDB, modifier `populate_synthetic` pour générer des payloads selon la distribution de convention. Sinon, accepter que T5 mesure la latence d'index/lookup à payload constant et que la calibration BlobDB se fera séparément (probablement Phase 3, avec un benchmark dédié W2).

Je pencherais pour la séparation. T5 actuel a un objet clair (la latence du lookup causal). Y ajouter de la variabilité de payload mélange deux questions et complique l'interprétation.

---

## 6. C2 — Hardware serveur à qualifier

### 6.1 État honnête du calcul actuel

C2 = 1 000 agents × 50 MB = 50 GB à recharger après cold start. Sur i3en.xlarge à 768 MB/s mesuré, c'est 65 s ; sur un NVMe « hypothèse serveur » à 3 GB/s, c'est 17 s. L'I/O Admission Control à 15 agents/s s'appuie sur le 768 MB/s.

Deux problèmes empilés :

Le 768 MB/s est mono-thread QD=1 (Trou n° 3 §1.2). Le vrai débit i3en.xlarge multi-thread est probablement 2,5–3 GB/s. Donc l'I/O Admission Control à 15 agents/s est sous-dimensionné d'un facteur 3–4 *sur le hardware déjà testé*.

Le « hardware serveur cible » est hypothétique. Tu n'as pas de serveur PCIe 4 dédié, donc le 3 GB/s n'est pas mesuré chez toi. Tu utilises un chiffre constructeur indicatif, ce que le protocole §3.1 interdit explicitement pour les manifests publiés (« Le champ `storage_seq_read_mb_s` doit être mesuré sur le périphérique utilisé »).

### 6.2 Recommandation : deux bornes, un cap actif

Découpler le **plafond planifié** (objectif de dimensionnement futur) du **cap actif** (ce que le scheduler enforce aujourd'hui), et reposer les deux sur des mesures honnêtes.

**Cap actif Phase 2/3 :** dimensionné sur le hardware réellement testé, après requalification fio multi-thread. Si fio multi-thread donne 2,5 GB/s sur i3en.xlarge, alors `cap_active = 50 agents/s` (et non 15). À documenter dans `spec/07` avec le numéro fio précis.

**Plafond planifié pour hardware serveur cible :** reste hypothétique. Le marquer explicitement comme tel dans `spec/07`. Formulation possible : *« Sur hardware serveur de référence à confirmer (NVMe PCIe Gen4, ≥ 5 GB/s mesuré multi-thread), C2 attendu à 100 agents/s. Hypothèse non testée — sera validée si et quand le hardware est disponible. »*

### 6.3 Conséquence sur le scheduler Phase 3

Le scheduler doit consommer le cap actif, pas le plafond planifié. C'est un paramètre de configuration, pas un constant compilé. Ça permet de relever le cap au fur et à mesure que le hardware est qualifié, sans rebuild.

### 6.4 Question subsidiaire : le « 50 MB / agent » est-il mesuré ?

C2 multiplie par 50 MB l'état d'un agent. Ce chiffre vient de W1 (ContentStore), mais est-ce mesuré sur un agent réel ou est-ce un objectif de design ? Si c'est un target, C2 a deux variables hypothétiques empilées (taille agent + débit hardware), et la borne actuelle est trop fragile pour servir de fondation au scheduler.

À clarifier dans `reference-workload.md` : statut de « 50 MB / agent ».

---

## 7. Ordre suggéré

Tout ce qui précède peut s'enchaîner dans une séquence où chaque étape débloque la suivante sans avoir à redécider à mi-parcours.

**Étape 1 — Nettoyer le run T5 existant (½ journée).** Appliquer les trois fixes §2 (drop_caches, sha256 source, fio multi-thread), faire un run propre supplémentaire, écrire L44. Coût minime, gain : un run de référence honnête sur lequel s'appuyer pour tout le reste.

**Étape 2 — Trancher Q1 (½ journée, pas de code).** Décomposer P3 en P3a/b/c dans `spec/02-properties.md`, fixer les cibles, marquer P3c réservée. Ça verrouille ce que T5-bis devra mesurer et évite de courir après « validé » sur la mauvaise quantité.

**Étape 3 — Trancher Q2 et Q3 ensemble (1 journée).** Documenter les deux modèles de working set dans `reference-workload.md`, documenter la distribution de convention `emit_payload`, déclarer `min_blob_size = 4 KB` provisoire dans un amendement à ADR-0017. Ces deux décisions sont couplées (Q2 conditionne le pattern d'accès qui informe BlobDB), il vaut mieux les prendre en une seule séance.

**Étape 4 — Requalifier C2 (½ journée).** Avec le fio multi-thread du §1, recalculer C2 pour cap actif, séparer plafond planifié hypothétique. Clarifier le statut du « 50 MB / agent ».

**Étape 5 — Seulement après : seconde instance pour « validé ».** `i3en.metal` est le candidat naturel — même famille, sans virtualisation, accès aux capteurs hardware donc première application opérationnelle du protocole §8 thermique. Coût AWS modeste sur quelques heures de bench.

**Étape 6 — En parallèle de l'étape 5 : T5-bis (P3b end-to-end).** Bench qui mesure `emit→fsync→get` plutôt que `get` isolé. Hardware identique à T5 existant. Donne la première donnée sur la portée (b) qu'aucune mesure actuelle ne couvre.

L'ensemble fait environ 1 semaine de travail focalisé. À la fin, H-causal-latence est partiellement validée *honnêtement* sur P3a (deux instances) et P3b (une instance), les fondations de C2 et de BlobDB sont reposées sur du mesuré, et le projet peut reprendre sereinement sur les chantiers Phase 3 (scheduler, primitives A5+).

---

## Annexe — Récapitulatif des décisions à prendre

| Décision | Forme | Effort | Bloque |
|---|---|---|---|
| Q1 — décomposition P3a/b/c | mise à jour `spec/02-properties.md` | ½ jour | T5-bis, classification validé |
| Q2 — modèles A & B working set | mise à jour `reference-workload.md §W1` | ½ jour | T5 multi-pattern, P3c |
| Q3 — distribution `emit_payload` | mise à jour `reference-workload.md` + amendement ADR-0017 | ½ jour | activation BlobDB Phase 3 |
| C2 — séparation cap actif / plafond planifié | mise à jour `spec/07` | ½ jour | scheduler Phase 3 |
| Fixes T5 (drop_caches, sha256, fio multi-thread) | code harness + re-run | ½ jour + 30 min bench | classification honnête |
| L44 — capitalisation T5 AWS | rédaction `lab/LESSONS.md` | 1 heure | mémoire institutionnelle |

Aucune de ces décisions n'invalide ce qui a été construit. Toutes resserrent la base empirique avant la prochaine vague de chantiers.