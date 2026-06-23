# Bundle compagnon — blog-02 « La flèche entre deux décisions EST un hash »

Carte du bundle. Deux objets distincts, à ne pas confondre :

| Dossier / fichier | Statut | Quoi |
|---|---|---|
| `REPRODUCE.md` | **(A) preuve** | Reproduction autoritaire sur le **vrai système**, cloné au tag. C'est la preuve. |
| `expected/` | **(A) preuve** | Transcripts réels (à capturer au tag, avec provenance : commit / tag / host / date). |
| `illustration/` | **(B) illustration** | Snippet auto-portant du *principe* (adressage par contenu + tamper-evidence), **sans** RocksDB ni le runtime. Pédagogique, **pas** une preuve (R-blog-5). |
| `figures/` | figure | Source de la figure de l'article (+ `SOURCES.md`). |

Règle : le code exécutable canonique vit ici ; l'article le **cite** (`@tag`), ne le duplique pas. La passe de preuve vérifie l'égalité article ↔ bundle avant chaque tag.
