# T5 bundle — procédure complète : préparer, transférer, lancer, récupérer

**Prérequis locaux :** `ssh`, `scp`, `tar`  
**Prérequis instance :** Ubuntu 22.04+ ou Amazon Linux 2023 — le script installe le reste

---

## Pourquoi ne pas utiliser `git archive`

`benchmarks/t5-bundle/` n'est pas commité dans git (statut `??`). Un `git archive HEAD`
ne l'inclut donc pas du tout. On utilise un `tar` ciblé à la place.

---

## 1. Préparer le bundle localement

Depuis la racine du repo :

```bash
# Variables à adapter une fois
KEY=~/.ssh/ma-cle.pem
HOST=ubuntu@ec2-xx-xx-xx-xx.compute-1.amazonaws.com   # ou ec2-user@ sur Amazon Linux

# Créer l'archive des seules choses nécessaires au bench
tar -czf /tmp/t5-bundle.tar.gz \
    --exclude='poc/target' \
    --exclude='results' \
    --exclude='.git' \
    poc/ \
    benchmarks/t5-bundle/
```

Ce que ça inclut : le source Rust (`poc/`) et les scripts du bundle (`benchmarks/t5-bundle/`).  
Ce que ça exclut : `target/` (binaires compilés, lourd), `results/` (déjà sur la machine locale), `.git/`.

Taille attendue : < 1 MB.

---

## 2. Copier le bundle sur l'instance

```bash
scp -i "$KEY" /tmp/t5-bundle.tar.gz "$HOST:~/"
```

---

## 3. Se connecter et décompresser

```bash
ssh -i "$KEY" "$HOST"
```

Une fois sur l'instance :

```bash
mkdir -p ~/os
tar -xzf ~/t5-bundle.tar.gz -C ~/os/

# Vérifier la structure
ls ~/os/benchmarks/t5-bundle/   # doit afficher run.sh, hardware_probe.sh, software_probe.sh
ls ~/os/poc/                     # doit afficher Cargo.toml, causal-log/, etc.

# Vérifier que le NVMe est visible
lsblk -d -o NAME,MODEL,SIZE | grep -i nvme
```

---

## 4. Lancer le benchmark

```bash
bash ~/os/benchmarks/t5-bundle/run.sh
```

Le script fait tout dans l'ordre :
1. Installe les dépendances système (apt/dnf)
2. Installe rustup si absent
3. Détecte et monte le NVMe instance store AWS
4. Vide le page cache OS (`sync` + `drop_caches=3`) — obligatoire pour un régime honnête
5. Mesure le hardware : fio QD=1 (coût unitaire) + fio QD=32 (capacité hardware)
6. Lance `cargo bench --bench causal_lookup` avec N=10⁸ sur le NVMe (~7 min)
7. Produit `~/os/results/T5/<timestamp>/` + archive `~/os/t5-results-<timestamp>.tar.gz`

En fin de run, il affiche la commande `scp` exacte avec le nom horodaté de l'archive.

**Variables optionnelles :**

```bash
# Test rapide sur N=10⁶ (quelques secondes)
BENCH_N=1000000 bash ~/os/benchmarks/t5-bundle/run.sh

# Forcer le répertoire de bench (si NVMe non détecté automatiquement)
T5_BENCH_DIR=/mnt/nvme1/t5 bash ~/os/benchmarks/t5-bundle/run.sh

# Sauter l'install paquets (re-run sur instance déjà préparée)
SKIP_INSTALL=1 bash ~/os/benchmarks/t5-bundle/run.sh
```

---

## 5. Récupérer l'archive de résultats

Depuis votre machine locale (nouveau terminal, instance encore active) :

```bash
# Le nom exact est affiché par le script — forme générique :
scp -i "$KEY" "$HOST:~/os/t5-results-*.tar.gz" ./
```

---

## 6. Intégrer les résultats dans le repo local

```bash
# Depuis la racine du repo local
tar -xzf t5-results-<timestamp>.tar.gz -C results/T5/

# Vérifier les points clés
cat results/T5/<timestamp>/workload.json
cat results/T5/<timestamp>/software.json
cat results/T5/<timestamp>/hardware.json
```

Points à vérifier :

| Fichier | Champ | Valeur attendue |
|---|---|---|
| `workload.json` | `drop_caches_applied` | `true` |
| `workload.json` | `cache_regime` | `"cache-miss-dominant"` si RAM < dataset/5, sinon `"cache-mixte"` |
| `software.json` | `source_tree_sha256` | non null — hash de traçabilité du source |
| `hardware.json` | `storage_seq_read_mb_s_qd1` | débit séquentiel mono-thread (base cap actif C2 ContentStore) |
| `hardware.json` | `storage_seq_read_mb_s_qd32` | débit séquentiel multi-thread (convergent avec QD=1 sur i3en.xlarge) |
| `hardware.json` | `storage_rand_read_iops_qd1` | IOPS aléatoires 4K QD=1 (métrique P3a et C2 — harness v4+) |
| `hardware.json` | `storage_rand_read_iops_qd32` | IOPS aléatoires 4K QD=32 (capacité hardware — harness v4+) |

Puis ajouter le run à `results/T5/SYNTHESE.md`.

---

## Temps estimés

| Étape | Durée |
|---|---|
| Préparation archive locale | < 10 s |
| Transfert vers instance | < 30 s |
| Install dépendances (première fois) | 3–5 min |
| Compilation Rust (première fois) | 5–8 min |
| fio QD=1 + QD=32 | ~45 s |
| Bench N=10⁸ | ~7 min |
| Récupération archive | < 30 s |
| **Total première fois** | **~20 min** |
| **Total re-run (`SKIP_INSTALL=1`)** | **~8 min** |

---

## Checklist avant de démarrer l'instance

- [ ] Archive `/tmp/t5-bundle.tar.gz` créée et taille < 1 MB
- [ ] L'instance est une `i3en.xlarge` (ou similaire avec NVMe instance store)
- [ ] Security group : port 22 ouvert depuis votre IP
- [ ] Clé `.pem` disponible localement
