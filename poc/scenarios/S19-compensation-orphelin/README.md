# S19 - Compensation journal orphelin (UC-12 / ADR-0024 D3)

**Regime :** R1 (P6 - atomicite crash, propriete d effet)
**Substrat :** Linux. Non transferable a seL4 (D7).

---

## Ce qui est teste

**P6** : CompensationOpen (0x11) sans CompensationClose (0x12) simule un crash entre
les deux. L oracle en ligne (replique de os-poc-reconstruct) detecte l orphelin.
Aucun etat partiel observable dans le ContentStore.

La simulation de crash est directe : ecriture manuelle de 0x11 dans le log du
scheduler sans appel a 0x12, sans crash processus. Cela correspond au cas
`rollback.after_compensation_open` (ADR-0024 D2) vu du log reconstruit.

### Oracle

- `open_set` non vide apres parcours du log scheduler (orphelin detecte)
- `ContentStore` de l agent inchange (dernier snapshot stable, pas d etat partiel)
- Interpretation : l agent repartirait de son etat pre-rollback au redemarrage

---

## Protocole

    1. Agent A construit un historique (snapshot S0 via barrier+emit).
    2. Simulation crash apres 0x11 :
         ecriture manuelle de LogEntry CompensationOpen (0x11) dans le log scheduler
         [target_agent_id][target_seq] en payload
         PAS d ecriture de CompensationClose (0x12)
    3. Detection orphelin (oracle inline) :
         open_set = []
         pour chaque entree du log scheduler :
           0x11 -> open_set.push(target_agent_id)
           0x12 -> open_set.remove(target_agent_id)
         => open_set non vide = orphelin detecte
    4. Verification P6 :
         ContentStore.last_snapshot de A == snapshot stable (S0)
         Aucun etat intermediaire observable

---

## Ce qui n est PAS teste

- Politiques de reconciliation detaillees (classification 0x11 seul / 0x11+0x0E / etc.) : ADR-0024 D3.
- os-poc-reconstruct binaire : la logique est testee inline.
- Power-loss (page cache perdu) : S15.

---

## Comment relancer

    cd poc
    CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
      -- tests::s19_compensation_orphelin --exact --nocapture

---

## References

- ADR-0024 D1/D2/D3 : Journal de compensation, points d injection, reconciliation.
- ADR-0027 : Durabilite log (CompensationOpen non-durable suffisant sous SIGKILL).
- S16 (UC-10) : Chemin nominal complet (0x11 -> 0x0E -> 0x0B -> 0x12).
