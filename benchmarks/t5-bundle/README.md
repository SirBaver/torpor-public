# T5 bundle — qualification H-causal-latence sur AWS one-shot

**Instance recommandée :** `i3.xlarge` (4 vCPU, 30.5 GB RAM, 950 GB NVMe local) ou `i3.2xlarge`. AMI : Ubuntu 22.04 LTS ou Amazon Linux 2023.

**Lancement (une commande après SSH) :**

```sh
git clone https://github.com/<owner>/<repo>.git && cd <repo> && bash benchmarks/t5-bundle/run.sh
```

**Sortie :** `t5-results-<timestamp>.tar.gz` à la racine du repo (< 1 MB). La commande `scp` exacte est affichée en fin de run.

**Note design :** le bundle vit dans le repo (pas d'archive séparée) — les sources évoluent ; cloner depuis git évite la désynchronisation. Le clone HTTPS prend quelques secondes ; aucun secret requis.
