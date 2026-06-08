# ADR-0048 — Jalon C.11 : chargement d'un module WASM non confié sur le JIT durci seL4

**Date :** 2026-05-29
**Statut :** Acceptée (cadrage — code en session dédiée)

---

## Contexte

C.10 (W^X du pool JIT, ADR-0047) et C.10-crash (atomicité crash dans la fenêtre de remap RW→RX) sont **PASS**. ADR-0047 §D2 a explicitement renvoyé le chargement d'un module **non confié** à un jalon distinct :

> Charger du non-confié (vecteur T1 réel) soulève sa propre grappe de décisions non tranchées (validation de provenance, configuration sandbox, limites de ressources) — c'est C.11, jalon distinct avec son modèle de menace.

Le déclencheur **T1** d'ADR-0047 §D2 (« exécution de WASM non confié prévue court terme ») est confirmé. C.11 est ouvert.

### Stack actuelle (jalons vivants)

- `poc/sel4-hello/c10-wx/` : runtime Wasmtime no_std seL4 AArch64, W^X actif (pool JIT = 128 frames dédiées, RW+XN en état W, R+exécutable en état X — ADR-0047 §D3).
- `poc/sel4-hello/c10-crash/` : kill-point dans la fenêtre de remap (atomicité préservée).
- `poc/sel4-hello/sel4-common/` : crate commune (child_vspace, object_allocator).
- `poc/sel4-hello/c10-wx/server/` : store redb/virtio-blk (reopen vérifié, ADR-0046 D-reopen).
- Isolation au **niveau processus seL4** (ADR-0037) : chaque runtime = un VSpace distinct ; le serveur de store survit à tout crash runtime (P6).

### Faits structurels déterminants pour le cadrage

Trois faits, vérifiés dans le code actuel, contraignent les réponses ci-dessous. Les ignorer produirait un jalon qui valide autre chose que ce qu'il prétend.

**F1 — Le runtime seL4 n'exécute aucun JIT à l'exécution.** `runtime/Cargo.toml` : `wasmtime = { features = ["runtime"] }` — **sans** `cranelift`. La compilation Cranelift a lieu au `build.rs` (host) ; le runtime fait `Module::deserialize(&engine, AGENT_CWASM)` sur un `.cwasm` AOT (`main.rs:90-92`). Conséquence : le « pool JIT » durci en W^X est en réalité le pool où le **code natif déserialisé** est copié puis exécuté. Le vecteur « Cranelift compile du code adversarial » **n'existe pas dans le PoC seL4** : Cranelift ne tourne pas sur la cible. Un `.wat` adversarial passe par Cranelift **sur l'hôte de build**, pas sur seL4.

**F2 — Le module est embarqué dans l'ELF du runtime.** `main.rs:39` : `static AGENT_CWASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/agent.cwasm"))`. Le `.cwasm` est dans le segment de données de l'ELF runtime, mappé par le superviseur. La provenance actuelle est donc **uniquement** « trusted (dans l'ELF) ». Le ring buffer (`get_ring_va`, `main.rs:49`) sert au commit Q3-C, **pas** au chargement de module. Aucun chemin de chargement « depuis le ring » n'existe aujourd'hui.

**F3 — Tout trap WASM panique le runtime entier (dette S4).** `platform.rs:177-180` : `wasmtime_longjmp` fait `panic!("wasmtime trap")`. `wasmtime_setjmp` (`:162-175`) n'enregistre aucun contexte et appelle directement le callback. Tout trap (OOB, div0, `unreachable`) remonte en `panic` du runtime → le VSpace runtime est tué par seL4. Le serveur de store survit (isolation processus). C'est le comportement actuel, cohérent avec le scope « modules triviaux » d'ADR-0037, mais **C.11 exécute précisément du WASM susceptible de trap.**

### Modèle de menace applicable (spec/08 §99-148)

- **Adversaire** (§101-111) : un agent compromis exécutant du WASM arbitraire dans son sandbox Wasmtime. Il **ne peut pas** exécuter de code natif, ni accéder à la mémoire WASM d'un autre agent, ni appeler une host function non importée. Surface = host functions importables.
- **Contexte seL4 PoC** : le runtime c10-wx n'expose **qu'une** host function — `emit` (`main.rs:96-133`). Les host functions Linux capability-gated (`agent_store_get/put`, `agent_check_cap`) **ne sont pas portées** sur seL4. C.11 s'exécute donc dans ce contexte réduit : surface adverse = `emit` uniquement.

---

## Décision

### D1 — « Non confié » au sens C.11 = provenance non garantie au chargement (Q1 = option C, déclinée)

« Non confié » se décompose en deux axes que le PoC actuel confond, et qu'il faut séparer pour ne pas valider un vœu :

1. **Axe provenance** (d'où vient le module) : aujourd'hui « dans l'ELF runtime » (F2) = trusted par construction. Le pendant non-trusted est « le module arrive d'un canal que le superviseur ne contrôle pas » — dans le PoC sans réseau, le seul canal de ce type est un **ring buffer / frame partagée** alimentée par un producteur tiers.
2. **Axe contenu** (que fait le module) : un module **adversarial** au sens du modèle de menace = OOB memory access, div0, `unreachable`, boucle non bornée, appels répétés à `emit`.

**Tranché :** C.11 vise l'option **C** (un module au contenu adversarial, chargé via un canal non-trusted), mais **décliné en deux sous-jalons** pour ne pas masquer une régression derrière l'autre (piège L68 : un happy path vert ne prouve pas la propriété) :

- **C.11 (cœur)** : axe **contenu** seul. Le module adversarial reste embarqué dans l'ELF (chargement inchangé, F2) mais son **contenu** est adversarial (OOB, div0, boucle). On démontre l'**isolation et la terminaison** sous WASM hostile. C'est le risque réel et levable maintenant.
- **C.11-prov** (sous-jalon, même session ou suivante) : axe **provenance**. Le `.cwasm` est chargé depuis une **frame partagée alimentée par le superviseur** (et non plus `include_bytes!`), `Module::deserialize` opérant sur des octets dont le runtime ne garantit pas l'origine. On démontre qu'un `.cwasm` **malformé/tronqué** est rejeté proprement (erreur `Module::deserialize`, pas corruption).

**Justification du refus de l'option A pure (ring sans contenu adversarial) :** charger un module trusted via un ring ne démontre que de la plomberie de chargement — pas une propriété de sûreté. **Justification du refus de l'option B pure (`.wat` adversarial embarqué) :** sans varier la provenance, on ne touche jamais au chemin `deserialize` sur octets non garantis, qui est le vrai vecteur supply-chain (spec/08 §15, §101-103). L'option C couvre les deux ; le découpage évite qu'un échec de plomberie de provenance masque un défaut d'isolation de contenu (ou l'inverse).

**Limite de scope explicite — pas de vérification de signature.** Le PoC ne signe ni ne vérifie cryptographiquement le `.cwasm`. La « provenance » C.11 se réduit à la distinction binaire **trusted (ELF) vs non-trusted (canal partagé)** + robustesse de `deserialize` face à des octets arbitraires. La validation de provenance cryptographique (signature, attestation supply-chain) est **hors scope** et renvoyée à C.12+ (voir D6). Raison : sans OS réseau ni PKI dans le PoC, une signature serait une plomberie sans modèle de menace réel à valider (on ne validerait pas une propriété, on simulerait une cérémonie).

### D2 — Critère opérationnel **C11_PASS**

C11_PASS exige les **quatre** points suivants. Chacun est un test observable, pas une affirmation.

> **P-α (isolation mémoire sous OOB)** — Un module dont `run()` exécute un accès mémoire hors des bornes de sa mémoire linéaire est **arrêté sans corrompre le runtime au-delà de ce que seL4 confine**. Critère observable : soit Wasmtime produit un trap `out of bounds memory access` (chemin propre, voir D3), soit — comportement S4 actuel — le trap panique le runtime et **seL4 tue le VSpace runtime via la sortie debug** ; dans les deux cas, **le serveur de store survit et répond** (smoke post-crash : un commit ultérieur d'un runtime sain réussit, seq monotone préservé).

> **P-β (terminaison sous boucle non bornée)** — Un module dont `run()` contient une boucle qui ne rend jamais la main est **terminé de l'extérieur en temps borné**, sans figer le serveur de store ni les autres composants. Mécanisme : voir D4. Critère observable : le superviseur observe la terminaison du runtime (fault/suspend) dans une borne temporelle définie, et le serveur reste réactif.

> **P-γ (durabilité du store sous crash provoqué par WASM)** — Les données commitées avant l'exécution du module adversarial restent intactes et relisibles après la mort du runtime. Critère observable : reopen du store (analogue D-reopen, ADR-0046) après le crash P-α/P-β → les Records pré-crash sont relus, seq cohérent.

> **P-δ (rejet d'un `.cwasm` non-trusted malformé — sous-jalon C.11-prov)** — Un `.cwasm` tronqué/corrompu chargé depuis le canal non-trusted (D1) provoque un échec **détecté** de `Module::deserialize` (le runtime signale l'erreur et ne tente pas d'instancier), sans exécuter d'octets arbitraires comme code. Critère observable : `Module::deserialize` retourne `Err`, le runtime émet un diagnostic et se suspend proprement (pas de VM fault d'exécution sur des octets non vérifiés).

**Test négatif obligatoire (cohérent ADR-0047 §D3 critère 2, piège L68) :** P-α et P-δ sont des **contre-exemples** au sens strict : un module qui *devrait* être confiné. Un C11_PASS qui ne montrerait que l'exécution d'un module adversarial « inoffensif » (sans réel OOB déclenché) ne prouve rien.

### D3 — La dette S4 (trap = panic) : **acceptable pour C11_PASS, à condition de l'inscrire au critère**, setjmp/longjmp réel renvoyé à C.12+

Question tranchée : faut-il implémenter un vrai `setjmp`/`longjmp` (S4) avant C.11 ?

**Non — pour le cœur C.11.** Le comportement actuel (trap WASM → `panic` runtime → seL4 tue le VSpace, serveur survit) **satisfait P-α** : l'isolation visée par C.11 est l'**isolation de processus seL4** (ADR-0037), pas la récupération intra-runtime. Un module OOB qui tue son propre runtime sans toucher le serveur **démontre exactement** la propriété T1 du modèle de menace (un agent compromis ne peut nuire qu'à lui-même). Implémenter un setjmp/longjmp réel n'ajoute aucune propriété de **sûreté** à C.11 ; il ajoute une propriété de **disponibilité/résilience** (le runtime survit au trap d'un de ses modules et peut en charger un autre) — utile en multi-agent par VSpace, mais ADR-0037 a acté 1 VSpace = 1 runtime, donc tuer le VSpace au premier trap est cohérent.

**Condition impérative :** le critère C11_PASS (D2 P-α) doit **nommer explicitement** que la terminaison passe par la mort du VSpace runtime, pas par un trap récupéré. Sinon on validerait « Wasmtime isole » alors qu'on observe « seL4 tue le processus » — deux propriétés distinctes (L68 / soundness ADR-0047). Le verdict C.11 doit écrire : *« l'isolation démontrée est S1b confinée par S1-processus seL4, pas la récupération de trap Wasmtime intra-runtime »*.

**Renvoi C.12+ :** le vrai setjmp/longjmp (S4) devient un déclencheur réel **quand** un runtime héberge plusieurs agents dans un même VSpace (ADR-0037 §Q-seL4-3 prévoit N agents / VSpace) **et** qu'on veut qu'un trap d'un agent ne tue pas les autres. C'est le moment où « trap = mort du VSpace » devient inacceptable. Tant qu'on est 1 agent / VSpace, S4 reste une dette FAIBLE (TODO §S4), désormais **requalifiée** : « atténuée par C.11 (isolation processus démontrée sous trap réel), à lever quand N agents / VSpace ».

### D4 — Terminaison de la boucle non bornée (P-β) : watchdog seL4, **pas** de fuel Wasmtime

Question tranchée (Q2/Q5) : fuel Wasmtime ou watchdog seL4 ?

**Tranché : watchdog externe via seL4 (préemption + suspend par le superviseur), pas le fuel Wasmtime.** Raisons :

1. **Le fuel n'est pas robuste comme borne de sûreté.** `Config::consume_fuel(true)` exige que le runtime appelle `Store::set_fuel` et gère le trap `out of fuel` — c'est une **coopération** du runtime, pas une contrainte externe. Un runtime compromis (ou un module qui désactiverait le yield) contourne le fuel. La boucle non bornée est précisément le cas où l'on ne veut pas dépendre de la coopération de l'exécuté.
2. **seL4 fournit déjà le mécanisme robuste.** Le superviseur détient la cap **TCB** du runtime. Une préemption externe (timer → le superviseur `tcb_suspend` le runtime après une borne) est la borne **non contournable** : elle ne dépend pas du code exécuté. C'est cohérent avec ADR-0037 (isolation au niveau processus) et avec le modèle watchdog déjà acté côté Linux (ADR-0025 profils watchdog, `EPOCH_TICK_MS_BASE`).
3. **État du fuel en no_std non vérifié — et non sur le chemin critique.** Le fuel reste **optionnel** comme borne *interne coopérative* (terminaison gracieuse d'un module bien élevé), mais il **ne peut pas** porter le critère P-β. Si l'on veut l'activer en complément, c'est un *plus*, pas le mécanisme de sûreté. Le critère P-β repose **uniquement** sur le watchdog seL4.

**Sous-question à instruire en session de code (non bloquante pour le cadrage) :** le PoC seL4 n'a pas encore de **source de temps** (timer/IRQ) câblée dans le superviseur. Deux options à chiffrer alors :
   - (a) Timer matériel (IRQ timer AArch64 `virt`) → le superviseur arme un timeout, reçoit l'IRQ, `tcb_suspend`. Robuste, plus de plomberie (cap IRQHandler, Notification).
   - (b) Pour le **smoke** C.11 uniquement : un thread superviseur qui boucle/`Yield` un nombre borné de fois puis `tcb_suspend` le runtime (borne en « tours d'ordonnancement », pas en temps mur). Suffit à *démontrer* P-β sans câbler un timer ; à ne pas confondre avec une vraie politique de watchdog temporelle.

   **Tranché au niveau cadrage :** P-β est validable par l'option (b) pour C.11 (démonstration de terminaison externe). La borne **temporelle** réelle (timer) est renvoyée avec la politique watchdog seL4 à C.12+. Le verdict C.11 doit dire « terminaison externe démontrée », pas « watchdog temporel borné à N ms ».

### D5 — Limites de ressources sandbox (Q2) : ce qui est dans le scope C.11

Configurables et **dans le scope** C.11 :
- **Mémoire linéaire** : `Config` / limites de `Store` (`StoreLimits` / `limiter`) bornent la croissance de la mémoire linéaire WASM. À configurer pour qu'un module ne puisse pas demander une mémoire linéaire débordant le budget VSpace runtime. **Note F1/platform :** `wasmtime_mprotect` est no-op hors pool JIT (`platform.rs:86-90`), donc la mémoire linéaire WASM n'est pas dans le pool durci — sa borne est la limite de `Store`, pas le W^X.
- **Stack** : `Config::max_wasm_stack` borne la profondeur d'appel WASM (protège contre récursion non bornée → débordement de la pile native du runtime). À vérifier que la valeur tient dans la pile allouée au TCB runtime.

**Hors scope C.11** (renvoi C.12+) : fuel comme politique de quota d'instructions par agent (lié à l'équité ADR-0023, non substrat tant que mono-agent/VSpace), limites de table/instances multiples.

### D6 — Découpage et renvoi à C.12+

| Jalon | Portée | Valide | Mécanisme |
|-------|--------|--------|-----------|
| **C.11 (cœur)** | Module **contenu adversarial** (OOB, div0, boucle), chargement ELF inchangé | P-α (isolation/OOB), P-β (terminaison/boucle), P-γ (store survit) | Trap→panic→seL4 tue VSpace (D3) ; watchdog externe `tcb_suspend` (D4) ; limites Store mémoire/stack (D5) |
| **C.11-prov** | `.cwasm` depuis **canal non-trusted** (frame partagée superviseur), incluant `.cwasm` malformé | P-δ (rejet `deserialize` propre, pas d'exécution d'octets arbitraires) | `Module::deserialize` sur octets non garantis ; échec détecté |
| **C.12+** (futur) | setjmp/longjmp réel (S4) ; watchdog **temporel** (timer IRQ) ; vérification de **signature**/attestation ; fuel comme quota d'équité ; N agents / VSpace | Récupération intra-runtime ; borne temps mur ; provenance cryptographique | Déclencheurs : N agents par VSpace (S4, fuel-équité), OS réseau/PKI (signature) |

**Déclencheurs C.12+ explicites :**
- **S4 (setjmp/longjmp réel)** ← première session où un VSpace runtime héberge ≥ 2 agents et où la mort d'un agent sur trap ne doit pas tuer les autres.
- **Watchdog temporel (timer)** ← besoin d'une borne en temps mur (SLA, équité E3 ADR-0023 portée sur seL4), ou multi-agent où le partage CPU exige une préemption temporelle.
- **Signature/attestation** ← introduction d'un canal de chargement réseau ou d'un second producteur de modules non mutuellement confiant (cohérent avec le déclencheur B-fort dormant, ADR-0036 §sortie : second TenantId).

### D7 — P6 / protocole de commit inchangé : smoke suffit (cohérent ADR-0047 §D6)

C.11 n'introduit **aucun** changement au protocole de commit (format Record, `seL4_Call` atomique, ordre Q3-C par ring SPSC). L'invariant ADR-0043 §71 est préservé. Re-jouer les 4 kill-points Q3-C est redondant. **P-γ se valide par smoke** (reopen post-crash, analogue D-reopen ADR-0046). Condition (identique §D6) : si C.11 modifiait le protocole de commit (non prévu), P6-crash complet redeviendrait obligatoire.

---

## Justification

### Pourquoi un ADR propre plutôt qu'un amendement d'ADR-0047

ADR-0047 décide le **durcissement W^X** (propriété de protection mémoire intra-VSpace, indépendante de la provenance du module). C.11 introduit trois décisions structurantes **nouvelles** : (1) la définition opérationnelle de « non confié » dans un PoC sans réseau (D1) ; (2) l'arbitrage isolation-de-processus vs récupération-de-trap, qui requalifie la dette S4 (D3) ; (3) l'arbitrage watchdog-seL4 vs fuel-Wasmtime comme borne de terminaison (D4). Ces trois décisions touchent le **modèle de menace** (surface adverse = `emit`, vecteur supply-chain `deserialize`) et requalifient une dette. Elles méritent un ADR propre.

### Le fait qui désamorce la moitié du risque anticipé

La grappe « provenance / config sandbox / limites ressources » annoncée par ADR-0047 §D2 supposait implicitement un **JIT à l'exécution** compilant du code adversarial. F1 montre que **Cranelift ne tourne pas sur la cible seL4** (`features=["runtime"]` sans `cranelift`). Le vecteur « Cranelift compile un module hostile sur seL4 » est donc **inexistant dans le PoC** : l'adversaire fournit un `.cwasm` AOT, et la surface est (a) le contenu sémantique du module à l'exécution (P-α/P-β), (b) la robustesse de `Module::deserialize` face à des octets arbitraires (P-δ). C'est un périmètre nettement plus étroit que « durcir un JIT face à du code hostile ». Le pool W^X protège l'intégrité du code natif **déserialisé**, pas un JIT-time adversarial.

---

## Conséquences

**Positives :**
- Lève le risque T1 (exécution de WASM non confié) sur substrat vivant, avec un critère à 4 propriétés observables.
- Requalifie la dette S4 : d'« à traiter avant d'exécuter du WASM susceptible de trap » à « atténuée par isolation processus démontrée, à lever quand N agents / VSpace » — clarifie son vrai déclencheur.
- Établit le pattern « borne de terminaison = capability TCB du superviseur, pas coopération de l'exécuté ».

**Négatives :**
- C.11 nécessite un (ou des) module(s) WASM adversarial(aux) dédié(s) au build (OOB, div0, boucle) — nouveaux artefacts de test.
- C.11-prov exige de sortir `AGENT_CWASM` de l'`include_bytes!` vers une frame partagée alimentée par le superviseur — modification du provisionnement (chemin de chargement nouveau).
- La borne de terminaison P-β en smoke (option b, tours d'ordonnancement) n'est **pas** une borne temporelle réelle ; le verdict doit le dire pour ne pas sur-vendre la propriété.

---

## Questions tranchées dans cet ADR (étaient bloquantes)

1. **Que signifie « non confié » ?** → Option C déclinée (D1) : C.11 cœur = contenu adversarial (axe contenu), C.11-prov = canal non-trusted + `.cwasm` malformé (axe provenance). Pas de signature (renvoi C.12+).
2. **Config sandbox / limites ressources ?** → Mémoire linéaire (`StoreLimits`) + stack (`max_wasm_stack`) dans le scope (D5) ; fuel-quota et limites avancées renvoyés C.12+.
3. **Provenance ?** → Distinction binaire trusted (ELF) / non-trusted (canal partagé) + robustesse `deserialize` ; suffit pour C.11 (D1). Cryptographie hors scope.
4. **Critère C11_PASS ?** → 4 propriétés observables P-α/P-β/P-γ/P-δ, avec test négatif obligatoire (D2).
5. **Fuel no_std ou watchdog ?** → Watchdog seL4 (`tcb_suspend` via cap TCB superviseur) porte P-β ; fuel reste optionnel, jamais critère de sûreté (D4). Source de temps réelle (timer) renvoyée C.12+.
6. **(implicite, S4)** **Trap = panic acceptable ?** → Oui pour C.11, à condition de l'inscrire au critère (l'isolation démontrée est S1-processus seL4, pas récupération de trap intra-runtime) ; setjmp/longjmp réel → C.12+ quand N agents / VSpace (D3).

---

## Séquencement

**Cadrage (cet ADR) maintenant ; code en session(s) dédiée(s).** Les six points ci-dessus devaient être tranchés avant la première ligne (modèle de menace, dette S4 requalifiée, mécanisme de terminaison — pas de la plomberie). Ordre suggéré pour la session de code : C.11 cœur (P-α/P-β/P-γ sur module embarqué) d'abord, puis C.11-prov (P-δ, chargement depuis frame partagée). Ne pas enchaîner avant validation de cet ADR.

---

## Références

- `decisions/0047-jalon-c10-wx-jit-sel4.md` §D2 (précondition C.11), §D3 (def. W^X + test négatif obligatoire), §D6 (smoke P6 si protocole inchangé)
- `decisions/0037-stack-runtime-sel4.md` §Q-seL4-3 (N agents / VSpace, isolation S1b sandbox WASM), §4 (trap WASM = signaux plateforme)
- `decisions/0043-integration-verticale-c6.md` §71 (invariant protocole de commit), §97 (risque CNode racine)
- `decisions/0046-scope-phase-9.md` (D-reopen — analogue P-γ)
- `decisions/0025-profils-watchdog-wasm.md` (modèle watchdog Linux — analogue D4), `decisions/0023-equite-formelle.md` (E3, fuel-équité renvoyé C.12+)
- `decisions/0036-autorité-causale-agent-add-cause.md` §sortie (déclencheur B-fort / second TenantId — lié au déclencheur signature C.12+)
- `spec/08-modele-menace.md` §15 (vecteur supply-chain `.wasm`), §99-111 (adversaire T1), §113-124 (host functions non-gated), §130-148 (T1 bypass capability)
- `poc/sel4-hello/c10-wx/runtime/src/main.rs:39` (`include_bytes!` cwasm = F2), `:90-92` (`Module::deserialize` = F1), `:96-133` (host function `emit` = surface adverse)
- `poc/sel4-hello/c10-wx/runtime/src/platform.rs:86-90` (mprotect no-op hors pool), `:162-180` (setjmp no-op / longjmp panic = S4 / F3)
- `poc/sel4-hello/c10-wx/runtime/Cargo.toml:18` (`features=["runtime"]` sans cranelift = F1)
- `lab/LESSONS.md` L68 (capacité de brique ≠ propriété démontrée), L83 (durcissement mémoire suit la détention de caps), L84 (VmAttributes, fault_ep)
- `TODO.md` §S4 (trap WASM = panic runtime — requalifiée par cet ADR)
- [Saltzer & Schroeder 1975] "The Protection of Information in Computer Systems" — complete mediation (la borne de terminaison ne doit pas dépendre de la coopération de l'exécuté)
