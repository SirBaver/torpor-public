<!-- Corps de la GitHub Release `blog-06-crash-sel4` (releases jumelles FR/EN). ⟨à épingler⟩ au tag. -->

# blog-06 — Couper le courant à 4 moments précis, 40 fois

**Régime :** R1 (effets) · **Statut :** prouvé (P6) · **Substrat :** seL4 / QEMU AArch64 — Wasmtime no_std / **redb** / virtio-blk (R-blog-2 : moteur **redb**, PAS RocksDB)

## Claim → Preuve

| Claim de l'article | Élément | Preuve (permalink `@blog-06-crash-sel4`) | Substrat |
|---|---|---|---|
| Atomicité crash : commit complet OU absent | 40 scénarios / 4 kill points | ⟨poc/sel4-hello/c10-crash/test.py⟩ + transcript | seL4/QEMU |
| Serveur survivant détecte l'incohérence | oracle au redémarrage | ⟨docs/demo/sel4-transcripts/⟩ | seL4/QEMU |
| W^X matériel (vm fault) | `C10_NEG_PASS` | ⟨docs/demo/sel4-transcripts/c10-wx-phaseA.txt⟩ | seL4/QEMU |

## Reproduire (exige la toolchain seL4)
```bash
git clone <url> && cd os-public/poc/sel4-hello && git checkout blog-06-crash-sel4
./demo-isolation.sh    # build + boot QEMU AArch64 + test W^X
```
> Sans la toolchain seL4, l'illustration accessible est le **transcript réel** + la figure (assumé).

## Limites (piège #2 / R-blog-3/4)
- « Démontré sur seL4 » = **QEMU AArch64, pas matériel de production** ; prototype de recherche.
- Le « kill » est un `tcb_suspend`, **pas une coupure de courant** ; power-loss = hors périmètre (ADR-0065/0046). « Committed » ≠ durabilité média.
- **Aucune performance seL4 revendiquée** (correction, pas latence).
