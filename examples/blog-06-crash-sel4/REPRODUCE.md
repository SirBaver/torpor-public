# Reproduction autoritaire — blog-06 (exige la toolchain seL4)

**Substrat :** seL4 / QEMU AArch64 — Wasmtime no_std / **redb** / virtio-blk. **Prototype de recherche, pas matériel de production.**
**Propriété :** P6 — atomicité crash (40 scénarios, 4 kill points ; le « kill » est un `tcb_suspend`, pas une coupure de courant). Power-loss = hors périmètre (ADR-0065/0046).

```bash
git clone <url-os-public> && cd os-public/poc/sel4-hello
git checkout blog-06-crash-sel4
./demo-isolation.sh        # build + boot QEMU AArch64 + test négatif W^X (C10_NEG_PASS)
# Atomicité crash : harnais déterministes c6-crash/ c7-crash/ c10-crash/ (test.py)
```

> Pour la plupart des lecteurs, l'illustration accessible **sans** la toolchain seL4 est le transcript réel capturé (`docs/demo/sel4-transcripts/`) + la figure de l'article — pas une réexécution. C'est assumé.
