# ADR-0046 — Scope Phase 9 : consolidation de la persistance seL4

**Date :** 2026-05-29  
**Statut :** Acceptée

---

## Contexte

À la clôture du jalon C.8 (ADR-0045), deux dettes architecturales ont été identifiées par review architect :

- **D-P3a** : la latence P3a n'a jamais été mesurée sur média réel *sous seL4* (QEMU virtio-blk n'est pas recevable comme substrat de mesure de latence — il est adossé au page cache hôte).
- **D-reopen** : la persistance redb n'a jamais été démontrée sur seL4. Le harness C.8 wipe `disk.img` avant chaque run ; le serveur ne redémarre jamais. Le chemin write→arrêt→reopen→read n'a jamais été exercé.

Par ailleurs, plusieurs items différés restent ouverts depuis des ADR antérieurs :
- GC des orphelins redb (ADR-0038 §Q3, différé)
- N > 2 agents dynamiques (ADR-0044 D1)
- Power-loss / β (ADR-0045 Q2=α, ADR-0038 §Phase 9, ADR-0027)

La question est : que contient Phase 9 ?

---

## Décision

### Phase 9 = consolidation de la persistance seL4 démontrée

Phase 9 n'est **pas** « lever α » (power-loss). La décision Q2=α a été prise le jour même (ADR-0045, 2026-05-29), justifiée par ADR-0027 et ADR-0038. Rien n'a changé qui justifie de la révoquer. De plus, lever α exigerait un harnais kill-QEMU et un modèle précis de ce que virtio-blk garantit à la barrière sous QEMU — or QEMU virtio-blk = page cache hôte, donc kill-QEMU ne simule pas une coupure d'alimentation réelle. Valider power-loss sur QEMU serait une validation trompeuse.

### Critère de sortie obligatoire

**D-reopen — smoke test persistance seL4**

Protocole : phase A = écriture de K paires (K ≥ 100) + ack `Committed` + arrêt propre du serveur (pas crash) ; phase B = relance du serveur sur le **même** `disk.img` (sans `dd`) + ouverture du redb existant + read des K paires + oracle bytewise.

Régime : crash-processus conforme à α — aucun power-loss requis. Le test doit être distinct du harness crash (qui conserve le wipe par KP pour l'isolation entre kill-points).

Phase 9 ne peut pas être déclarée close sans ce test PASS.

### Items optionnels (déclencheur observable requis)

- **GC orphelins redb** : à implémenter si D-reopen révèle une croissance non bornée du store après plusieurs cycles write→arrêt→reopen. Différé conforme à ADR-0038 §Q3.
- **N > 2 agents dynamiques** : à traiter si un besoin concret l'exige. Faible valeur épistémique par rapport au coût — P6-N et I4 sont déjà validés à N fixe (ADR-0044). Différé conforme à ADR-0044 D1.

### Hors scope Phase 9 — renvoyé Phase 10+

- **Power-loss / β** : requiert un harnais kill-QEMU avec modèle précis de sémantique de barrière virtio-blk sous QEMU (ou substrat réel), et un ADR de cadrage dédié. Décision conforme à ADR-0045 Q2=α, ADR-0038 §B2/B3, ADR-0027 D3.
- **D-P3a** (latence P3a sur média réel sous seL4) : nécessite une board réelle ou NVMe passthrough. Non bloquant pour Phase 9. À traiter quand le substrat matériel est disponible.

---

## Justification

### Pourquoi D-reopen est bloquant

Sans smoke test de réouverture, « store persistant » est une affirmation de construction (redb écrit sur bloc), jamais d'observation (redb relit le bon état). C'est la même classe de dette que L68 (C5_PASS = capacité de brique, pas validation de propriété). Le mot « persistant » dans le titre de C.8 ne devient honnête qu'après D-reopen PASS.

### Pourquoi power-loss reste Phase 10+

Power-loss est un modèle de menace distinct de crash-processus. Il met en jeu l'ordre d'écriture sur média, les caches du contrôleur bloc, et la sémantique de barrière de virtio-blk — différente d'un disque physique. Sous QEMU, kill-QEMU tue le processus hôte mais le page cache hôte peut avoir déjà fait le flush, ce qui ne reproduit pas une coupure d'alimentation réelle. Valider β correctement demande un harnais et un modèle de faute distincts, plus un ADR propre. Ouvrir β en Phase 9 sans ce travail produirait une validation trompeuse.

---

## Conséquences

- **TODO.md** : créer section Phase 9 avec D-reopen (bloquant), GC-orphelins et N>2 (optionnels conditionnés), power-loss (Phase 10+).
- **ADR-0045** : non amendé par cet ADR — les amendements Q1/Q2 sont dans ADR-0045 lui-même.
- **ADR-0038** : non amendé — §Phase 9 (power-loss) reste inchangé.
- **ADR-0027** : non amendé — la règle de symétrie fsync (D4) s'appliquera si et quand α est levé.
- **ADR-0044** : non amendé — D1 (N>2 dynamique) reste différé.

---

## Références

- `decisions/0045-critere-completude-poc-sel4.md` Q2=α, amendements Q1/Q2 (2026-05-29)
- `decisions/0038-store-natif-sel4.md` §Q3 (GC orphelins différé), §B2/B3 (Phase 9 power-loss)
- `decisions/0027-durabilite-log-vs-contentstore.md` D3 (régimes SIGKILL vs power-loss), D4 (symétrie fsync)
- `decisions/0044-integration-verticale-c7.md` D1 (N>2 dynamique différé)
- `poc/sel4-hello/c8-store/test.py` lignes 22-26 (wipe disk.img — origine de la dette D-reopen)
- `lab/LESSONS.md` L79 (QEMU virtio-blk valide le fonctionnel, pas la latence), L80 (commit ≠ persistance)

---

## Amendement 2026-05-29 — D-reopen : chemin réel = kill-QEMU post-writes-synchrones

**Contexte :** §34 spécifie « phase A = … arrêt propre du serveur (pas crash) ». L'implémentation (`poc/sel4-hello/c9-reopen/test.py`, `child.terminate(force=True)`) tue QEMU de force ; le serveur reste figé dans `ep.recv()`, jamais arrêté coopérativement.

**Constat :** le résultat est valide et l'intention de D-reopen est satisfaite. Les writes virtio sont synchrones → les blocs ont atteint `disk.img` (page cache hôte) avant le kill. Phase B rouvre le même `disk.img` (sans `dd`), lit les K=100 entrées, oracle bytewise PASS (`verified=100, seq_a=100`). Le chemin de relecture est exercé.

**Décision :** on ACTE que le smoke test D-reopen démontre la **durabilité niveau 1 sous kill du processus QEMU post-writes-synchrones**, plutôt qu'après arrêt coopératif du serveur. Ce chemin est plus exigeant qu'un arrêt propre (un arrêt propre laisserait redb flusher ses buffers applicatifs, ce qui pourrait masquer un défaut de durabilité du chemin chaud ; le kill ne le permet pas). On ne corrige PAS le protocole vers un arrêt coopératif : il n'existe pas de mécanisme de drain de la boucle `ep.recv()`, et il n'aurait aucune valeur épistémique supplémentaire pour α.

**Garde-fou (régime α maintenu) :** le kill-QEMU ici NE valide PAS power-loss / β. Sous QEMU, le page cache hôte a déjà absorbé les writes synchrones ; un vrai power-loss mettrait en jeu l'ordre d'écriture média et la sémantique de barrière virtio-blk — non testés. Le test reste strictement dans le régime crash-processus α (cf. §28, §60, ADR-0045 §54). Ne pas présenter C.9 comme touchant β.

**Référence :** `poc/sel4-hello/c9-reopen/test.py:49,61,70` ; LESSONS.md L80 (commit ≠ persistance, exercer le reopen) ; ADR-0045 amendement Q2.
