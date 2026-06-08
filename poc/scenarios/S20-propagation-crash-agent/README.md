# S20 - Propagation erreur cross-agent (UC-13 / ADR-0015)

**Regime :** R1 (P3/P4 - tracabilite + isolation, proprietes d effet)
**Substrat :** Linux. Non transferable a seL4 (D7).

---

## Ce qui est teste

**P3/P4** : quand agent A se termine (arret propre, canal ferme), B qui depend
causalement de A :
- Conserve le lien causal vers la derniere action de A (parent_ids integre).
- Ne recoit pas de message orphelin de A apres sa terminaison.
- Peut continuer a operer independamment.

### Oracle

- B.log : entree B1 avec A1_id dans parent_ids (causalite preservee)
- A.log : Lifecycle Spawned + Active (terminaison propre, pas de AgentCrash)
- Canal de A ferme : `tx_a.send(..)` retourne Err apres terminaison

---

## Acteur(s)

| Nom | Source | Role |
|-----|--------|------|
| Agent A | CROSS_AGENT_WAT | Produit A1, puis se termine (canal ferme) |
| Agent B | CROSS_AGENT_WAT | Recoit A1 comme cause, continue independamment |

---

## Protocole

    Agent A                        Agent B
    msg[0]=0 -> barrier+emit -> A1
    A1_id = entries_by_agent(A).last().0

    test envoie Message::caused([0x00], A1_id) a B
                                   B traite -> B1
                                   parent_ids(B1) contains A1_id

    drop(tx_a) -> run_loop A se termine (inbox fermee)
    wait 100ms

    ORACLE:
      B1.parent_ids contains A1_id         (causalite cross-agent preservee)
      tx_a.send(..) == Err                  (canal A ferme)
      A.log: Lifecycle Spawned + Active(s)  (pas de AgentCrash)

---

## Ce qui n est PAS teste

- AgentCrash (0x13) via watchdog : t_algo_profile_traps_at_100ms couvre.
- Messages de A en attente dans l inbox de B au moment du crash : atomicite canal mpsc garantie.
- Supervision asymetrique post-crash : S1.

---

## Comment relancer

    cd poc
    CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
      -- tests::s20_propagation_crash_agent --exact --nocapture

---

## References

- ADR-0015 : Propagation erreur cross-agent, AgentCrash (0x13).
- ADR-0003 : Modele causal DAG, parent_ids cross-agent.
- Message::caused : constructeur message avec cause cross-agent.
