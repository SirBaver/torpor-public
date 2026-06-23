# Transcripts attendus (à capturer au tag)

Ces transcripts sont **capturés au moment du tag** `blog-02-dag-causal`, à partir d'un vrai run (pas fabriqués). Chaque fichier porte un en-tête de provenance :

```
# provenance
# commit : <sha>            (= tag blog-02-dag-causal)
# host   : <cpu> / <nvme>   (substrat de mesure, R-blog-1)
# date   : <YYYY-MM-DD>
# commande : <commande exacte de REPRODUCE.md>
```

Fichiers à produire :
- `log-verify.exit1.transcript.txt` — vérificateur sur un journal corrompu d'une entrée (exit 1 + action_id désignée).

> Un transcript sans provenance est invérifiable — exactement le grief de l'article 2 contre les logs éditables. On ne commet pas ici le péché qu'on dénonce.
