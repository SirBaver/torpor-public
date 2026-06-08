# Verdict orphan-metric

Date: 2026-06-02
Reference: ADR-0055 D4
Verdikt: PASS

## Resultats

K=200 M=500: blocks=700 headers=500 Delta=200 attendu=200 ecart=0 PASS
K=5000 M=50000: blocks=55000 headers=50000 Delta=5000 attendu=5000 ecart=0 PASS
K=0 M=1000: blocks=1000 headers=1000 Delta=0 attendu=0 ecart=0 PASS

## Conclusion

Sur store frais, estimate-num-keys est exact.
Reserve empirique ADR-0055 D4 levee pour regime store frais.
Reserve residuelle: compaction active, a valider au premier run GC.

## Substrat

AMD Ryzen 5 PRO 4650U, Linux, RocksDB 8.10
