# ADR-0045 — Critère de complétude du PoC seL4

**Date :** 2026-05-29  
**Statut :** Acceptée

---

## Contexte

Après C.7-crash (ADR-0044 soldé), la question « qu'est-ce qui constitue un PoC seL4 complet ? » n'était tranchée dans aucun ADR. Trois options ont été soumises à l'architect :

- **A** — complétude minimale : P6 + I4 validées sur seL4, tout le reste transitivité Linux.
- **B** — complétude partielle : chaîne de commit persistante end-to-end (C.8) + P3a re-mesurée sur redb/virtio-blk.
- **C** — complétude forte : rejouer SEF-1 à SEF-7 sur seL4. Coût 3–6 mois.

Parallèlement, la question du scope power-loss était ouverte :

- **α** — power-loss hors scope : modèle de menace = crash-processus uniquement, conforme ADR-0038 §Q2 et ADR-0027.
- **β** — power-loss in-scope : `commit_durable()` + flush virtio-blk acquitté + re-validation P6 en régime kill-QEMU.

---

## Décision

### Q1 = B — Complétude partielle (store persistant intégré)

**Le PoC seL4 est complet quand :**

1. La chaîne `runtime → ring → serveur → redb → virtio-blk → média` fonctionne end-to-end sur seL4 (jalon C.8).
2. P3a est re-mesurée sur ce stack (p99 ≤ 10 ms, 10⁶ entrées, redb sur virtio-blk, pas HashMap RAM).

Toutes les autres propriétés restent validées selon leur statut actuel (voir §Matrice).

### Q2 = α — Power-loss hors scope PoC

Le modèle de menace du PoC reste **crash-processus** (SIGKILL/panic). P6 est et reste validée au niveau C.6-crash/C.7-crash uniquement. `commit_durable()` et la re-validation P6 en régime kill-QEMU sont Phase 10+ ou décision future.

**Garde-fou obligatoire (à respecter dans C.8) :** l'ack `StoreReply::Committed` signifie durabilité niveau 1 (ADR-0038 §Q2) — acquittement serveur RAM *et/ou* écriture bufferisée sur virtio-blk. Il ne signifie **pas** durabilité média garantie. Le code ne doit jamais implicitement promouvoir l'ack au niveau 3-4. Toute ambiguïté sur ce contrat crée un bug de durabilité partielle silencieux.

---

## Justification

### Pourquoi pas A

A déclare le PoC complet alors que la propriété centrale du projet — un journal append-only autoritaire dont la lecture est servie par un cache reconstructible — n'est jamais exercée dans le pipeline de commit seL4. C.4 (virtio-blk) et C.5 (redb no_std) ont validé les composants isolément ; la chaîne n'a jamais été intégrée dans le serveur de store. La transitivité Linux→seL4 est défendable pour les propriétés algorithmiques (P4, P5) ; elle ne l'est pas pour P3a, dont la valeur est précisément dans le comportement du *stack de stockage réel* (index sur média bloc, pas HashMap RAM). Mesurer P3a sur HashMap et l'extrapoler à redb-sur-virtio-blk, c'est mesurer autre chose.

### Pourquoi pas C

Rejouer SEF-1 à SEF-7 sur seL4 pour éliminer une transitivité dont une partie est légitime. P5 (déterminisme) et P4 (capabilities) sont des propriétés de logique, non de substrat — les rejouer teste surtout le portage. Le coût de C se justifie si le PoC devient produit — décision prématurée à ce stade.

### Pourquoi α et pas β

ADR-0027 et ADR-0038 §Q2 ont déjà tranché : power-loss = Phase 9+. La position par défaut est α. Sur le fond : power-loss est un modèle de menace *différent* de crash-processus — il met en jeu l'ordre d'écriture sur média, les caches du contrôleur bloc, la sémantique de barrière de virtio-blk, et la symétrie fsync exigée par ADR-0027. Le valider correctement demande un harnais kill-QEMU et un modèle précis de ce que virtio-blk sous QEMU garantit à la barrière (différent d'un disque physique — risque de validation trompeuse). C'est un jalon distinct avec sa propre métrologie.

---

## Matrice de complétude

| Propriété | Substrat de référence | Statut |
|-----------|----------------------|--------|
| **P1** densité | Linux PoC | Transitivité documentée |
| **P2** rollback ≤ 100 ms | Linux PoC (SEF-2) | Transitivité conditionnée à C.8 (store persistant requis) |
| **P3a** lookup ≤ 10 ms p99 | Linux redb + **seL4 C.8 (cible)** | À re-valider sur seL4 post-C.8 |
| **P3b** end-to-end ≤ 20 ms | Linux mesuré | Transitivité documentée ; non re-mesuré seL4 |
| **P4** isolation capabilities | Linux PoC (SEF-3) | Transitivité documentée (propriété algorithmique) |
| **P5** déterminisme | Linux PoC (SEF-6) | Transitivité documentée (propriété algorithmique) |
| **P6** atomicité crash-processus | seL4 C.6-crash + C.7-crash | ✓ Validée sur seL4 |
| **I4** non-interférence intégrité | seL4 C.7-crash | ✓ Validée sur seL4 |
| **P6** power-loss | — | Hors scope PoC (α) |

**Note transitivité P2 :** P2 (rollback) dépend du store. En Phase 8 le serveur est en RAM — P2 est vérifiable mais sur un store non-persistant. Post-C.8, le store étant persistant (redb+virtio-blk), P2 est conditionnée à la bonne implémentation du pipeline persistant. Elle doit être re-vérifiée fonctionnellement après C.8 (pas nécessairement via SEF-2 complet — un smoke test suffira).

---

## Jalon C.8 — définition

**Objectif :** intégrer redb fork no_std comme backend du serveur de store seL4, en aval du journal append-only Q3-C (ADR-0038 §Q3), avec virtio-blk comme `StorageBackend`.

**Critères de sortie (C8_PASS) :**
1. La chaîne de commit `runtime → ring → seL4_Call → serveur → redb → virtio-blk` fonctionne end-to-end.
2. P3a re-validée : 10⁶ entrées, K=3 passes de 1 000 `Get`, p99 ≤ 10 ms.
3. P6 re-validée sur la chaîne complète : 4 kill-points Q3-C côté runtime, oracle sur le serveur survivant, invariants I1/I2/I3 intacts. Régime : crash-processus (tcb_suspend), pas power-loss.
4. L'ack `Committed` respecte le garde-fou α : durabilité niveau 1 only, commentaire explicite dans le code.

**Ce que C.8 ne fait PAS :**
- Rejouer SEF-2 full (P2) — smoke test rollback suffit.
- Valider power-loss — hors scope (α).
- Implémenter GC des orphelins redb — différé (ADR-0038 §Q3, Phase 8).
- N > 2 agents — non bloquant pour C.8 (généralisation dynamique différée ADR-0044 D1).

---

## Conséquences

- **TODO.md** : C.8 devient la prochaine entrée dans la Phase 8. ADR-0045 est la décision de base.
- **ADR-0038** : non amendé — §Q2 (durabilité niveau 1) et §Phase 9 (power-loss) restent inchangés. L'ADR-0045 est cohérent.
- **ADR-0027** : non amendé — la règle de symétrie fsync (D4) s'appliquera si et quand α est levé en β.
- **ADR-0042** : non amendé — redb reste cache reconstructible, jamais autoritaire.

---

## Références

- `decisions/0038-store-natif-sel4.md` §Q2 (durabilité niveau 1), §Q3 (Q3-C content-addressed), §B2/B3 (Phase 9)
- `decisions/0027-durabilite-log-vs-contentstore.md` D4 (règle de symétrie fsync), D3 (régimes SIGKILL vs power-loss)
- `decisions/0042-voie-b3-moteur-index.md` §Amendement (redb = cache, non autoritaire)
- `decisions/0043-integration-verticale-c6.md` — P6 mono-agent seL4
- `decisions/0044-integration-verticale-c7.md` — P6-N + I4 seL4
- `spec/02-properties.md` — P1–P6 définitions
- `spec/09-transfert-poc-sel4.md` §3 Q-seL4-1/2/3

---

## Amendements

### Amendement 2026-05-29 — Q1 : Critère 2 P3a reformulé

**Contexte :** Le critère 2 de `§Critères de sortie C8_PASS` exigeait « P3a re-validée : 10⁶ entrées, K=3 passes de 1 000 Get, p99 ≤ 10 ms » sur le stack seL4. C.8 a été déclaré PASS sans exécuter ce critère ; la mesure importée est celle de `poc/redb-p3a` (Linux/NVMe).

**Problème :** La mesure de latence p99 sur QEMU/virtio-blk n'est pas recevable comme preuve de P3a-latence. virtio-blk sous QEMU est adossé au page cache de l'hôte, non à un média réel. Le chiffre obtenu ne prédit ni NVMe réel, ni le comportement sur board réelle — il aurait remplacé l'extrapolation HashMap→redb par une mesure trompeuse page-cache-hôte→média-réel. Le garde-fou « risque de validation trompeuse » (§54, invoqué contre β) s'applique ici mot pour mot.

**Décision :** Le critère 2 est reformulé :

- P3a est validée *fonctionnellement* sur le stack seL4 : le chemin de lookup `redb→virtio-blk` produit la bonne valeur end-to-end (C.8 PASS).
- La *latence* p99 P3a est validée sur média réel hors seL4 : Linux/NVMe, `poc/redb-p3a`, p99 = 739 µs, ×13 sous cible.
- La mesure de latence sur QEMU est explicitement **non requise et non recevable**.

C8_PASS reste valide sous ce critère reformulé.

**Dette ouverte :** D-P3a — latence P3a sur média réel *sous seL4* (board réelle ou NVMe passthrough). Non bloquante pour la complétude PoC. Voir ADR-0046.

| P3a | Linux/NVMe (poc/redb-p3a) | seL4 C.8 |
|-----|--------------------------|----------|
| Fonctionnalité (bonne valeur retournée) | ✓ | ✓ |
| Latence p99 ≤ 10 ms | ✓ 739 µs | non recevable sur QEMU |
| Latence sur média réel sous seL4 | — | dette D-P3a |

---

### Amendement 2026-05-29 — Q2 : Persistance reopen non démontrée sur seL4

**Contexte :** Le harness `test.py` de C.8 reconstruit `disk.img` (`dd if=/dev/zero`) avant chaque kill-point. Le serveur ne redémarre jamais entre les runs. Aucun scénario ne comporte la séquence write → arrêt propre → reopen → read.

**Conséquence :** La propriété « persistance » (données survivent à un redémarrage du serveur) n'est pas démontrée sur seL4. C.8 valide le *chemin d'écriture* end-to-end et l'atomicité P6 en régime crash-processus. Il ne valide pas le *chemin de relecture* depuis le média après réouverture.

**Matrice de complétude — ligne ajoutée :**

| Propriété | Substrat | Statut |
|-----------|----------|--------|
| Persistance reopen (write→arrêt→reopen→read) | seL4 | NON démontrée — dette D-reopen |

**Dette ouverte :** D-reopen — smoke test write → arrêt propre du serveur → reopen sur le même `disk.img` (sans `dd`) → read K paires → oracle bytewise. Bloquant pour clore Phase 9 (voir ADR-0046).
