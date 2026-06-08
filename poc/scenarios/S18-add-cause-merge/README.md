# S18 - agent_add_cause legitime : noeud de merge (UC-1 / ADR-0003 / ADR-0008)

**Regime :** R1 (P3c - tracabilite causale concurrente, propriete d effet)
**Substrat :** Linux. Non transferable a seL4 (D7).

---

## Ce qui est teste

**P3c** : construction d un vrai noeud de merge (N>1 parents) dans le DAG causal
via `agent_add_cause`. Pendant legitime de SEF-7 (qui teste le refus de forgerie) :
S18 prouve qu un merge reel est correctement constitue et que parent_ids reflète
la causalite declaree.

### Oracle

- `parent_ids.len() >= 2` (noeud de merge, pas une action lineaire)
- `parent_ids` contient l action_id de A (cause externe declaree via add_cause)
- `parent_ids` contient l action_id precedente de C (chaine interne intacte)

---

## Acteur(s)

| Nom | Source | Role |
|-----|--------|------|
| Agent A | CROSS_AGENT_WAT | Produit l action A1 qui sera citee par C |
| Agent C | CROSS_AGENT_WAT | Appelle add_cause(A1) puis barrier -> merge node C1 |

---

## Protocole

    Agent A                        Agent C
    msg[0]=0 -> barrier+emit -> A1
    A1_id = entries_by_agent(A).last().0

                                   msg[0]=0 -> barrier+emit -> C0

                                   msg = [0x04] + A1_id (33 bytes)
                                      -> add_cause(A1) + barrier+emit -> C1
                                   parent_ids(C1) = [C0_id, A1_id]  <- merge N=2

    ORACLE:
      C1.parent_ids.len() >= 2
      A1_id in C1.parent_ids

---

## Ce qui n est PAS teste

- Forgerie (action_id invente) : SEF-7 couvre ce cas.
- N>2 causes simultanees (un seul add_cause par message dans CROSS_AGENT_WAT).
- Reconstruction via os-poc-reconstruct : S14 / P3a.

---

## Comment relancer

    cd poc
    CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
      -- tests::s18_add_cause_merge --exact --nocapture

---

## References

- ADR-0003 : Modele causal DAG, agent_add_cause.
- ADR-0008 : Causalite concurrente, noeuds de merge.
- SEF-7 : Pendant (refus de forgerie par agent_add_cause).
- poc/runtime/src/actor.rs::CROSS_AGENT_WAT
