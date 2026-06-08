# FINDING-B-5 — TCB Linux : noyau ~30 MLOC non prouve formellement

**Classe :** Surface d'attaque du TCB (Trusted Computing Base)
**Reference :** Linux kernel CVE database + [Klein 2009] seL4 formal verification
**Rejoue :** non (demonstration de concept non jouee)
**Regime :** R1
**Substrat :** Linux (D7)

---

## Taille et historique du TCB Linux

| Metrique | Linux | seL4 |
|---------|-------|------|
| LOC noyau | ~30 000 000 | ~9 000 (C) + ~600 000 (Isabelle) |
| CVEs noyau (2020-2025) | >2 000 | 0 (sur code prouve) |
| Privilege escalation (LPE) | Classe active (CVE reguliers) | Non applicable (preuve formelle) |
| Preuve de securite formelle | Non | Oui (information flow, integrity) |

Le noyau Linux est dans le TCB de toute application Linux. Un LPE (Local Privilege
Escalation) permet a un agent malveillant de sortir de tous les bacs a sable
applicatifs, y compris Wasmtime, les capabilities, et les mecanismes cgroups/namespaces.

## Scenarios de compromission TCB

**Non rejoues** (necessitent une CVE noyau activement exploitable) :

1. LPE via syscall : un agent WASM qui escape Wasmtime (B-1) peut appeler des
   syscalls arbitraires et exploiter une LPE noyau => ring 0 complet.
2. Container escape : dans un deploiement conteneurise, les namespaces Linux ne
   constituent pas une isolation de securite forte — plusieurs LPEs les ont traverses
   (e.g., runc CVE-2019-5736, CVE-2024-21626).
3. Memory tagging bypass : meme avec ARM MTE (Memory Tagging Extension), le noyau
   Linux lui-meme n'est pas tag-protege dans toutes les configurations.

## Position de ce projet (spec/08 §0.2)

ADR-0037 et spec/08 option alpha excluent Linux du TCB cible :
- Cible = seL4 microkernel (< 5 000 LOC C dans TCB)
- Linux reste le substrat de developpement/validation, avec la regle D7 :
  les verdicts Linux ne transferent pas a seL4.

## Comparaison seL4

seL4 est le seul OS grand public avec une preuve formelle de :
- Correctness fonctionnelle (implementation = specification)
- Integrite (un processus non autorise ne peut pas modifier la memoire d'un autre)
- Confidentialite (flow d'information controle)

La preuve couvre le code C et l'assembleur critique. Les 9 000 LOC C sont
exhaustivement specifies en Isabelle/HOL. Un LPE seL4 n'a pas ete demontre.

## Type de limite

**Structurelle** — taille et nature du TCB Linux. Non corrigeable par patch
applicatif. Necessite un changement de substrat.
