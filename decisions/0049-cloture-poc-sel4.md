# ADR-0049 — Clôture du PoC seL4

**Date :** 2026-05-30
**Statut :** Acceptée

---

## Contexte

Le PoC seL4 a été déclaré **complet au sens d'ADR-0045 Q1=B** (chaîne de commit persistante `runtime → ring → serveur → redb → virtio-blk` + P3a fonctionnelle), puis Phase 9 close par ADR-0046 (D-reopen PASS). Trois jalons de durcissement **au-delà** du critère de complétude ont ensuite été soldés, chacun déclenché par un risque réel et identifié :

- **C.10** (W^X du pool JIT Wasmtime, ADR-0047) — dette de soundness S1 (violation W^X).
- **C.11** (WASM non confié, contenu adversarial, ADR-0048 §cœur) — vecteur de menace T1.
- **C.11-prov** (axe provenance, `.cwasm` depuis canal non-trusted, P-δ, ADR-0048 §D1) — `C11PROV_PASS` 2026-05-30.

La question tranchée ici : **que faire après C.11-prov ?** Trois options ont été soumises (a) instancier la séparation CAS/index d'ADR-0038 §3 ; (b) ouvrir un ADR power-loss / β ; (c) clore le PoC seL4 et remonter au niveau spec.

### Fait structurel déterminant

Les items de durcissement restants ont tous des **déclencheurs objectifs non atteints** :

| Item | Déclencheur de réveil | Atteint ? |
|------|----------------------|-----------|
| setjmp/longjmp réel (ADR-0048 §D6, dette S4) | ≥ 2 agents par VSpace | Non |
| watchdog temporel (timer IRQ) | partage CPU multi-agent / SLA | Non |
| fuel comme quota d'équité (ADR-0023 E3) | multi-agent / VSpace | Non |
| signature / attestation supply-chain | réseau, PKI, ou second producteur de modules | Non |
| GC orphelins redb (ADR-0038 §Q3, ADR-0046 §42) | croissance non bornée du store observée sur cycles reopen | Non |
| power-loss / β (ADR-0045 Q2=α, ADR-0046 §47) | substrat média réel (board / NVMe passthrough) | Non |
| N > 2 agents dynamiques (ADR-0044 D1) | besoin concret | Non |

Le PoC seL4 a donc **épuisé ses déclencheurs objectifs**. Tout travail de code seL4 supplémentaire serait tiré par la propreté ou l'anticipation, pas par un risque à lever — ce qui est la définition de la violation YAGNI que le projet refuse déjà pour les items C.12+ et pour B-fort (ADR-0036 §sortie).

---

## Décision

### D1 — Le PoC seL4 est clos

Le PoC seL4 est déclaré **clos**. Il atteste, sur substrat seL4 AArch64 vivant (QEMU virt) :

- la complétude ADR-0045 Q1=B (chaîne de commit persistante + P3a fonctionnelle) ;
- la persistance reopen (D-reopen, ADR-0046) ;
- l'atomicité crash-processus P6 mono-agent et P6-N + non-interférence I4 (C.6-crash, C.7-crash, ADR-0043/0044) ;
- le durcissement mémoire W^X du pool JIT (C.10, ADR-0047) et son atomicité crash (C.10-crash) ;
- l'isolation de processus sous WASM non confié au contenu adversarial (C.11, P-α/P-β/P-γ) et la robustesse de `Module::deserialize` face à un canal de provenance non-trusted (C.11-prov, P-δ).

Aucun nouveau jalon de code seL4 n'est instruit. Les déclencheurs dormants ci-dessus (D3) restent ouverts mais non instruits.

### D2 — Inscription au récit de complétude : la séparation CAS/index est une **cible non instanciée**

C'est le cœur de cet ADR, et la raison pour laquelle (c) ne peut pas être « fermer et partir ».

Trois ADR vivants affirment l'invariant **« journal append-only content-addressed autoritaire + index reconstructible non-autoritaire »** comme propriété du store :

- ADR-0038 §3 : « l'index […] est un cache entièrement reconstructible […]. Il n'est jamais autoritaire. »
- ADR-0042 §Amendement : « redb reste cache reconstructible, jamais autoritaire. »
- **ADR-0045 §Justification (« Pourquoi pas A »)** : le rejet de l'option A de complétude s'appuie explicitement sur le fait que A « n'exerce jamais le journal append-only autoritaire dont la lecture est servie par un cache reconstructible » dans le pipeline seL4.

**Constat (L82, confirmé dans le code de tous les jalons C.6→C.11-prov) :** le serveur de store n'instancie **pas** cette séparation. `commit_to_redb` ouvre quatre tables redb — `TABLE_BLOBS`, `TABLE_HEADERS` (content-addressed) **mais aussi** `TABLE_JOURNAL_A` (seq→header_hash) et `TABLE_SEQ` — dans **une seule** transaction `begin_write()` / `wtx.commit()`. L'ordre (`TABLE_JOURNAL_A`) est de l'état autoritaire vivant dans redb, **non reconstructible** depuis les blobs CAS. L'atomicité observée (P6) est celle de la transaction redb englobante — une **sur-garantie** (ACID transactionnel ⊃ append atomique), pas l'append atomique sur un store CAS séparé que l'interface prétend porter.

**Décision :** on **promeut** ce constat du statut d'annexe (amendement ADR-0038 du 2026-05-29, leçon L82) au statut de **fait de clôture**. Le récit de complétude est rectifié comme suit :

> Les PASS C.6 → C.11-prov attestent **l'isolation 2-processus** (runtime ne touche jamais le store), **P6 / P6-N en régime crash-processus**, **I4**, **W^X**, et **l'isolation sous WASM non confié**. Ils **n'attestent pas** l'invariant « CAS autoritaire / index reconstructible non-autoritaire » d'ADR-0038 §3 et ADR-0042. Le store réel du PoC est un **store redb transactionnel monolithique**. La séparation spécifiée est une **cible architecturale non instanciée**.

**Conséquence sur ADR-0045 :** la justification du rejet de l'option A (§« Pourquoi pas A ») contient **deux** arguments indépendants — (1) A n'exerce pas l'invariant journal-autoritaire/cache-reconstructible ; (2) A extrapole une mesure HashMap RAM vers redb-sur-bloc (P3a), ce qui « mesure autre chose ». **Seul l'argument (1) est retiré** ; l'argument (2) reste valable et porte à lui seul le rejet de A. Précision de fidélité : ADR-0045 n'affirmait **pas littéralement** « B exerce l'invariant » — il le **suggérait par implicature** de l'opposition A/B (rejeter A *parce qu'*il n'exerce pas X suggère que B, lui, l'exerce). C'est cette implicature qui est retirée, pas une phrase existante : B ne l'exerce pas davantage que A. La complétude B tient par la chaîne de commit réelle exercée et par P3a fonctionnelle sur redb-sur-virtio-blk, **pas** par l'instanciation de cet invariant. ADR-0045 est **amendé** sur ce point précis — sa **conclusion (complétude B) reste valide** (voir §Conséquences).

### D3 — Déclencheurs dormants : conditions de réveil

Les directions (a) et (b) ne sont pas rejetées sur le fond — elles sont **différées faute de déclencheur**, au même titre que les items C.12+ :

- **(a) Instanciation de la séparation CAS/index** — déclencheur = **implémentation du GC des orphelins**, lui-même déclenché par croissance non bornée du store observée sur cycles reopen (ADR-0046 §42). Le GC suppose des blobs/headers orphelins distincts d'un index jetable, **incompatible** avec l'index couplé transactionnellement (L82 corollaire) — l'implémenter forcera la re-séparation. Tant que le GC n'est pas réclamé, (a) n'apporte **aucune propriété observable nouvelle** (P6 tient déjà par sur-garantie). C'est une dette d'**architecture spécifiée non instanciée**, pas une dette de soundness — elle se documente, elle ne se code pas en urgence.

- **(b) Power-loss / β** — déclencheur = **substrat média réel** (board ou NVMe passthrough). Sur QEMU, virtio-blk = page cache hôte ≠ média réel : valider β y serait une **validation trompeuse**, déjà interdite par ADR-0027 D3, ADR-0045 §54, ADR-0046 §60. Ouvrir un ADR β maintenant ne ferait que redire « non recevable sans matériel » — conclusion déjà actée (ADR-0046 §47). C'est un déclencheur **matériel**, pas une décision de direction.

- **(c) Upgrade Wasmtime ≥36.0.7/≥42.0.2/≥43.0.1** *(greffé 2026-06-03, red team B-1)* — déclencheur = **activation de `wasm_memory64`** (OU upgrade requis pour toute autre cause). `cargo audit` (2026-06-03) remonte 15 advisories actifs sur wasmtime 25.0.3, dont RUSTSEC-2026-0096 (CVSS 9.0, miscompilation Cranelift aarch64 → sandbox escape). L'advisory borne le scope : *« 32-bit WebAssembly is not affected »* — le CVE est **N/A par configuration** tant que `memory64` reste désactivé (défaut wasmtime, jamais configuré). Garde fail-closed : test `memory64_reste_desactive` (`poc/runtime/src/lib.rs`). Tant que le test passe, la dette est dormante ; son échec = `memory64` activé = upgrade requis. Pas d'ADR dédié (aucune décision de soundness active à encoder — cf. arbitrage architect 2026-06-03 : « N/A par configuration ≠ décision »). Voir `red-team/campagne-B-substrat/FINDING-B-1.md`.

### D4 — Direction post-clôture : remontée au niveau spec

La seule direction tirée par un besoin réel est la **consolidation au niveau spec** : synthèse de transfert, capitalisation des six ADR seL4 (0037–0048) et de la dizaine de leçons (L68–L86), et réconciliation de l'écart spec/code de D2.

**Garde-fou de remontée :** la synthèse de transfert **ne doit pas** décrire le store du PoC comme instanciant la séparation CAS/index. Elle doit porter la mention « cible non instanciée » de D2. Le texte de spec actuel est propre sur ce point — `spec/09:117` (Q-seL4-2) pose même la question comme **ouverte** (« comment garantir l'atomicité sans `WriteBatch` ? un WAL maison ? une structure append-only pure ? »). Il s'agit donc de **ne pas introduire** la fausse affirmation, pas de corriger un texte existant.

**Corollaire — Q-seL4-2 est à re-instruire, pas seulement à ne pas contaminer.** Le code a *de fait* répondu à la question spec « comment l'atomicité **sans** `WriteBatch` ? » par « via la transaction redb englobante » — qui **est** un WriteBatch sémantique. La prémisse de Q-seL4-2 (« sans `WriteBatch` ») est donc **caduque** : le choix d'implémentation l'a contredite. La remontée spec doit re-cadrer Q-seL4-2 (constater que l'atomicité est portée par la transaction redb, et que la séparation append-only pure reste la cible non instanciée de D2), pas seulement éviter d'y injecter une fausse affirmation.

---

## Justification

### Pourquoi (c) et pas (a) maintenant

(a) est techniquement la plus propre des trois — raison suffisante de s'en méfier comme moteur de décision. Elle n'a **pas son déclencheur** (GC non réclamé) et n'apporte **aucune propriété nouvelle**. Refuser C.12+ pour absence de déclencheur et accepter (a) sans le sien serait un deux-poids-deux-mesures. La distinction de L82 est dirimante : *propriété tenue ≠ architecture instanciée* ; une dette d'architecture sans déclencheur de propriété se trace, elle ne se code pas.

### Pourquoi (c) et pas (b) maintenant

(b) est l'option la mieux argumentée **contre**, par les ADR du projet eux-mêmes (0027/0045/0046). Sans matériel réel, il n'existe aucun substrat sur lequel β soit recevable. Ouvrir β maintenant produirait soit une validation QEMU trompeuse (interdite), soit un ADR redisant une conclusion déjà tranchée. Déclencheur matériel, pas décision de direction.

### Pourquoi un acte documentaire est bloquant avant la remontée spec

Sans D2, la remontée spec hériterait d'une incohérence : ADR-0045 — le verdict de complétude que liront les futurs agents — justifie partiellement la complétude par une propriété que le code ne réalise pas. L82 et l'amendement 0038 le constatent déjà, mais **enterrés** dans une leçon et un amendement, pas dans le récit de complétude. Les y laisser, c'est risquer que la synthèse de transfert sclérose L82 en fausse vérité acquise. D2 remonte le constat au niveau du verdict.

---

## Conséquences

- **ADR-0045** : **amendé** par cet ADR — la justification « Pourquoi pas A » est rectifiée : l'argument « B exerce l'invariant journal-autoritaire / cache-reconstructible » est retiré ; la complétude B repose sur la chaîne de commit réelle + P3a fonctionnelle, pas sur l'instanciation de l'invariant. La conclusion (complétude B) **reste valide**.
- **ADR-0038** : non amendé — §3 (invariant cible) et l'amendement Q3 (2026-05-29) restent. Cet ADR **promeut** leur constat au récit de clôture, sans révoquer la cible.
- **ADR-0042** : non amendé — redb reste cible « cache reconstructible » ; la non-instanciation est désormais inscrite.
- **ADR-0048** : non amendé — les déclencheurs C.12+ (§D6) sont repris en D3 sans changement.
- **ADR-0046** : non amendé — les déclencheurs GC / power-loss / N>2 sont repris en D3 avec leurs conditions.
- **spec/** : aucune correction *de fausse affirmation* requise ; garde-fou de remontée en D4. La remontée spec devra **re-instruire Q-seL4-2** (`spec/09:117`) dont la prémisse « sans `WriteBatch` » est rendue caduque par le choix d'implémentation (transaction redb englobante).
- **INDEX.md** : créer la ligne ADR-0049 (« amende 0045 §justification, pas conclusion ») ; renseigner la colonne « Amendé par » d'ADR-0045 avec « 0049 (retrait argument invariant ; conclusion B inchangée) ».
- **TODO.md** : section Phase 10 close pour la partie durcissement (C.10/C.11/C.11-prov soldés) ; les déclencheurs dormants D3 y figurent avec condition de réveil ; prochaine entrée = consolidation spec (D4).

---

## Questions tranchées dans cet ADR (étaient bloquantes)

1. **Direction post-C.11-prov ?** → (c) clore + remonter spec ; (a) et (b) différées faute de déclencheur (D1, D3, D4).
2. **Le store instancie-t-il la séparation CAS/index d'ADR-0038 §3 ?** → Non (L82, confirmé code). Inscrit au récit de clôture comme cible non instanciée ; ADR-0045 amendé (D2).
3. **Faut-il corriger la spec ?** → Non — le texte est propre (`spec/09:117` pose la question comme ouverte). Garde-fou : la synthèse ne doit pas introduire la fausse affirmation (D4).
4. **(a) et (b) sont-elles rejetées ?** → Non, différées avec déclencheurs explicites : (a) ← GC réclamé ; (b) ← matériel réel (D3).

---

## Références

- `decisions/0045-critere-completude-poc-sel4.md` §Justification « Pourquoi pas A » (point amendé par D2), Q1=B, Q2=α
- `decisions/0046-scope-phase-9.md` §42 (GC déclencheur), §47 (D-P3a matériel), §60 (power-loss validation trompeuse)
- `decisions/0038-store-natif-sel4.md` §3 (invariant cible), §Amendement Q3 2026-05-29 (constat non-instanciation)
- `decisions/0042-voie-b3-moteur-index.md` §Amendement (redb = cache reconstructible)
- `decisions/0048-jalon-c11-wasm-non-confie.md` §D6 (déclencheurs C.12+)
- `decisions/0027-durabilite-log-vs-contentstore.md` D3 (régimes SIGKILL vs power-loss)
- `decisions/0036-autorité-causale-agent-add-cause.md` §sortie (discipline YAGNI / déclencheur dormant)
- `lab/LESSONS.md` L68 (capacité de brique ≠ propriété), L82 (transaction ACID ≠ séparation instanciée), L86 (provenance non-trusted)
- `poc/sel4-hello/c11-prov/server/src/main.rs` fn `commit_to_redb` (4 tables, 1 transaction — état L82 confirmé)
- `spec/09-transfert-poc-sel4.md:117` (atomicité posée comme question ouverte)
