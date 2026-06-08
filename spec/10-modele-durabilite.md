# 10 — Modèle de durabilité

**Version :** 1.0 — 2026-05-30

---

## 1. Objet de ce chapitre

La durabilité du système est définie et contrainte par six ADR et un verdict de scénario : ADR-0027 (régime Linux), ADR-0038 §Q2 (régime seL4), ADR-0045 §Q2 (décision α/β), ADR-0046 (garde-fou QEMU), ADR-0049 §D3b (groupement des dettes infra), ADR-0051 §D4 (items #7b et #8 différés), et SEF-10/VERDICT (fenêtre cross-store, sévérité démontrée).

Ce chapitre consolide ces sources sans rouvrir leurs arbitrages. Il établit :

1. Une taxonomie à quatre niveaux de durabilité (D1–D4), applicables aux deux substrats.
2. Un tableau de couverture par substrat et régime de menace, avec les validations existantes.
3. L'invariant de cohérence cross-store **I-CSR**, définition opérationnelle unique.
4. La règle de symétrie fsync, applicable à toute future promotion de niveau.

---

## 2. Taxonomie — quatre niveaux de durabilité

Les niveaux suivent la chaîne `write → serveur de stockage → page cache OS → cache contrôleur → média`. Chaque niveau ajoute une frontière de survie.

| Niveau | Nom court | Données survivent si | Ne survivent pas si |
|--------|-----------|----------------------|---------------------|
| **D1** | RAM serveur de stockage | Le processus *runtime* (agent) crashe | Le processus *serveur de stockage* crashe, ou kernel panic, ou power-loss |
| **D2** | Page cache OS | Le processus crashe (SIGKILL/panic) via WAL noyau | Kernel panic, power-loss |
| **D3** | Cache contrôleur device | Le noyau crashe (si contrôleur avec PLP) | Power-loss sans PLP |
| **D4** | Média persistant | Power-loss | Défaillance hardware du média |

**Nota :** les niveaux sont cumulatifs — D2 ⊆ D3 ⊆ D4. D1 est un sous-ensemble de D2 sur seL4 (le serveur survit au crash du runtime, donc ses données RAM sont déjà « post-crash runtime ») mais non équivalent : D1 ne garantit pas que les données ont quitté la mémoire applicative du serveur vers le page cache.

**Niveau effectif Linux PoC (ADR-0027, observation SEF-4) :** D2 *à la marge* — RocksDB avec `WriteOptions::default()` bufferise applicativement le WAL avant le `write(2)` syscall. Les écritures qui ont atteint le page cache OS survivent ; celles encore dans le buffer interne RocksDB peuvent être perdues sous `process::exit(1)` sans destructeur. P6 (atomicité par action) tient par construction — aucun état partiel observé — mais le contrat « `append()` retourné OK ⇒ écriture visible post-SIGKILL » n'est pas garanti sans `bytes_per_sync`. Voir ADR-0027 §Observation post-décision et §D1.

**Niveau effectif seL4 PoC (ADR-0038 §Q2, ADR-0045 §garde-fou) :** D1 — `sync_data()` est un no-op ; les données sont dans la RAM du serveur de stockage (processus seL4 distinct, survive au crash du runtime). Ni flush driver, ni fsync, ni garantie power-loss.

---

## 3. Couverture par substrat et régime de menace

| Substrat | Niveau effectif | Crash-processus runtime (α) | Crash kernel / power-loss | Validation |
|----------|-----------------|-----------------------------|--------------------------|------------|
| **Linux PoC** RocksDB no-force | D2 (WAL OS-buffered) | Couvert — P6 tient | Non couvert | SEF-4, 40 runs, 4 kill-points (ADR-0027) |
| **seL4 PoC** redb / virtio-blk | D1 (RAM serveur) | Couvert — serveur survit au crash runtime | Non couvert | C6-crash, C7-crash, C8, C10-crash (ADR-0043, ADR-0044, ADR-0047) |

**Régimes explicitement hors scope (décision ferme) :**

- **Crash-kernel / kernel panic :** non couvert sur aucun substrat. Sur Linux, le page cache est perdu ; sur seL4, le serveur de stockage (in-TCB) est lui-même affecté.
- **Power-loss :** hors scope PoC, tranché Q2=α (ADR-0045). Déclencher uniquement sur substrat média réel (board physique, NVMe passthrough), groupé avec D-P3a et β-seL4 (ADR-0051 §D4, item #8). Valider sur QEMU serait une validation trompeuse (QEMU virtio-blk = page cache hôte, ADR-0046 §garde-fou, L32).

---

## 4. Invariant de cohérence cross-store — I-CSR

### 4.1 Définition opérationnelle

Après toute séquence `(écrire N actions → coupure → reopen)`, pour tout `log_entry` présent dans le journal persisté au moment du reopen :

```
I-CSR : ∀ log_entry ∈ journal : log_entry.snapshot_hash ∈ store
```

Autrement dit : tout `SnapshotHeader` référencé par le log causal doit exister dans le store content-addressed (ContentStore sur Linux, tables `TABLE_BLOBS`/`TABLE_HEADERS` dans redb sur seL4).

I-CSR est la condition nécessaire et suffisante pour que P2 (rollback) soit opérationnel à la reprise. Un log_entry dont le `snapshot_hash` est absent du store produit `Err(MissingBlock)` au rollback — état non-détecté silencieusement sans I-CSR (SEF-10 finding sévérité (b)).

### 4.2 Asymétrie orphelin / référence pendante

Les deux violations de I-CSR ne sont pas symétriques (ADR-0051 §D1, spec/02 §P6) :

| Cas | Nom | Admis par le modèle no-force | Impact sur P6 |
|-----|-----|------------------------------|---------------|
| Bloc dans le store **non référencé** par le log | **Orphelin** | **Oui** — le store peut être « en avance » sous D2/D1 (store écrit avant le log ; log_entry écrit en dernier) | Nul — bloc GC-able, aucun agent ne peut le référencer |
| `log_entry` référençant un snapshot **absent** du store | **Référence pendante** | **Non** — viole I-CSR | Casse P2 (rollback → `MissingBlock`) et P6 (état déchiré) |

Un orphelin est la conséquence normale du modèle d'écriture content-addressed : les blobs et headers sont écrits avant le log_entry qui les rend « visibles ». Une référence pendante ne peut survenir que si le log est persisté « en avance » sur le store — ce qui se produit sous cache-loss avec réordonnancement OS, si le WAL du CausalLog atteint le disque avant le WAL du ContentStore (SEF-10, L89). Cette fenêtre est la dette #7b (ADR-0051 §D4).

### 4.3 Détection au reopen (correctif #7a)

`restore_from_evicted` (`poc/runtime/src/actor.rs`) vérifie `last_snapshot ∈ store` avant d'adopter un état restauré. En cas d'absence, elle retourne `RuntimeError::Store(MissingBlock)` — fail-fast explicite au lieu d'adoption silencieuse. Ce correctif **détecte** la violation de I-CSR ; il ne **ferme pas** la fenêtre (fermeture = commit cross-store atomique, dette #7b, déclencheur : chantier GC / re-séparation CAS-index, ADR-0049 §D3a requalifié par ADR-0051 §D4).

### 4.4 Portée de I-CSR par substrat

| Substrat | I-CSR vérifiable | Fenêtre de violation | État |
|----------|-----------------|----------------------|------|
| **Linux PoC** | Oui — ContentStore et CausalLog sont des DB RocksDB séparées | Fenêtre cross-DB si log arrive sur disque avant store (cache-loss) | Détecté (#7a) ; fenêtre non fermée (#7b différé) |
| **seL4 PoC** | Oui — tables `TABLE_BLOBS`/`TABLE_HEADERS` et `TABLE_JOURNAL_A` dans la même transaction redb | Aucune fenêtre : transaction ACID unique couvre blobs + journal atomiquement (sur-garantie par rapport à I-CSR) | PASS C6-crash à C11-prov ; aucune dette I-CSR |

Sur seL4, l'atomicité réelle (transaction redb unique) est plus forte que I-CSR — elle empêche structurellement la référence pendante. Cette sur-garantie satisfait I-CSR par construction, mais ne réalise pas la séparation CAS-autoritaire / index-reconstructible d'ADR-0038 §3 (cible non instanciée, L82, ADR-0049 §D2).

---

## 5. Règle de symétrie fsync

**Toute promotion de niveau (D1 → D2 → D3 → D4) doit être appliquée symétriquement au store (blobs + header) ET au log.** Cette règle découle de I-CSR : rompre la symétrie introduit une fenêtre de référence pendante ou un état non traçable.

| Cas asymétrique | Effet | Violé |
|----------------|-------|-------|
| fsync log uniquement | Log « en avance » sur store sous cache-loss | I-CSR — référence pendante |
| fsync store uniquement | Store « en avance » sur log | Traçabilité P3/P4 — état committé non loggué |
| fsync des deux (D3→D4) | Cohérence garantie sous power-loss | — |

L'unité minimale de fsync symétrique est la **commit barrier d'action** : `put_block` + `put_snapshot` (ContentStore) + `append` du log_entry (CausalLog), dans cet ordre, avec barrière fsync après les deux. La transaction de compensation (`0x11`/`0x12`) ne porte pas d'écriture ContentStore — elle n'est pas concernée par la symétrie (ADR-0027 §D4).

**Coût indicatif (ADR-0027 §Coût) :** 0.5–15 ms de latence par fsync selon hardware. Sur le matériel de référence (NVMe PCIe Gen 3, classe 2), p99 fsync ≈ 5 ms (T5-bis). Une promotion D2→D3 sur le chemin chaud ferait passer la latence commit de ~20 µs (actuel) à ~5–15 ms — à arbitrer avec P1b (débit d'agents actifs) au moment du déclenchement.

---

## 6. Points d'extension futurs

Les items suivants sont différés avec déclencheur explicite (ADR-0051 §D4, ADR-0049 §D3b) :

| Item | Description | Déclencheur |
|------|-------------|-------------|
| **#7b** | Commit cross-store atomique (fermeture fenêtre I-CSR sur Linux) | Chantier GC / re-séparation CAS-index (ADR-0049 §D3a) |
| **#8** | Verdict durabilité power-loss (promotion D2→D4 Linux, D1→D4 seL4) | Substrat média réel (board physique ou NVMe passthrough) |
| **D-P3a** | Latence P3a sur NVMe réel sous seL4 | Même substrat que #8 |
| **β-seL4** | P6 en régime kill-QEMU + modèle barrière virtio-blk | Même substrat que #8 |
| **Séparation CAS/index** | Instanciation de l'invariant ADR-0038 §3 (journal autoritaire / index reconstructible) | GC orphelins (croissance non bornée observée, ADR-0046 §42) |

Ne pas anticiper ces items sans déclencheur atteint. En particulier, ne pas simuler power-loss par `drop_caches` ou kill-QEMU — validation trompeuse documentée (ADR-0046, L32).

---

## 7. Références

- `decisions/0027-durabilite-log-vs-contentstore.md` — modèle no-force, régimes SIGKILL/power-loss, règle de symétrie §D4, observation SEF-4
- `decisions/0038-store-natif-sel4.md` §Q2 — niveaux D1–D4 (terminologie IPC seL4), durabilité RAM serveur
- `decisions/0045-critere-completude-poc-sel4.md` §Q2 — décision α (power-loss hors scope PoC)
- `decisions/0046-scope-phase-9.md` §garde-fou — QEMU virtio-blk non recevable pour power-loss
- `decisions/0049-cloture-poc-sel4.md` §D2/§D3b — cible CAS/index non instanciée, groupement dettes infra
- `decisions/0051-cloture-campagne-tri-findings.md` §D4 — items #7b/#8, règle de symétrie fsync
- `poc/scenarios/SEF-10-cross-store-crash/VERDICT.md` — I-CSR, fenêtre cross-store, sévérité findings (a)(b)(c)
- `lab/LESSONS.md` L32 (simulation power-loss = piège), L82 (redb monolithique ≠ séparation CAS/index), L89 (référence pendante cross-store)
- `spec/02-properties.md §P6` — asymétrie orphelin/référence pendante, trou cross-store inscrit
- [Gray & Reuter 1992] *Transaction Processing* — no-force/steal, taxonomie durabilité

---

*Ce chapitre consolide ; il ne crée pas de nouvelles décisions. Tout arbitrage futur sur la durabilité doit passer par un ADR dédié et respecter la règle de symétrie §5.*
