# FINDING-6 — Flood `0x14` > 32 ressources distinctes

**Vecteur :** A-6  
**Propriété attaquée :** P4 (audit de refus)  
**Use case de référence :** UC-19 / S31  
**Régime :** R1 (effets)

---

## Hypothèse d'attaque

Un agent malveillant émet des demandes d'accès à > 32 ressources distinctes en rafale pour saturer le log d'entrées `0x14 CapabilityDenied`. Hypothèse : soit le log est saturé (DoS par volume), soit les entrées au-delà de 32 sont silencieusement abandonnées → trou d'audit exploitable pour dissimuler des accès.

## Oracle

`poc/scenarios/S31-audit-flood-beyond-32/VERDICT.md` — harnais Rust déterministe.

Invariant binaire (double) :
1. Les entrées émises n'atteignent pas un volume illimité (borne anti-DoS active).
2. Toute entrée au-delà de la borne est **signalée** par un sentinel, pas silencieusement abandonnée — le trou d'audit est lui-même auditable.

## Résultat

**LIMITE DOCUMENTÉE**

La borne de 32 entrées `0x14` distinctes par action est active (ADR-0051 §D2). Au-delà, le runtime émet un sentinel `F2` (type spécial dans le log) qui indique que le reste a été tronqué. Le sentinel est lui-même une entrée non-silencieuse : un auditeur qui lit le log voit `F2` et sait qu'il manque des entrées.

Ce comportement est **documenté et intentionnel** : la borne est anti-DoS délibérée. Un agent qui déclenche `F2` est identifiable comme anormal — le sentinel est une preuve de comportement suspect, pas une dissimulation.

## Classification

**Limite documentée (borne anti-DoS)** — aucun patch requis.

| Cas | Comportement |
|---|---|
| ≤ 32 refus distincts | Tous enregistrés dans le log |
| > 32 refus distincts | Entrées tronquées + sentinel F2 émis |
| Lecture log par auditeur | F2 visible → comportement suspect traçable |

## Notes

- ADR-0051 §D2 documente la décision de borne = 32. La valeur est paramétrable à la compilation, pas à chaud.
- Le vecteur d'attaque original (saturation log) est neutralisé par la borne. La variante (dissimulation via troncation) est neutralisée par le sentinel non-silencieux.
