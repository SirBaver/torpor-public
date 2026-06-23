ILLUSTRATION DU PRINCIPE — CE N'EST PAS LE RUNTIME TORPOR.
Ce dossier reproduit l'IDÉE de l'adressage par contenu + tamper-evidence en autonome
(sans RocksDB / Wasmtime / le DAG réel).
La PREUVE vit dans ../REPRODUCE.md (le vrai système, cloné au tag, mesuré sur Linux/RocksDB).
Ce code n'établit aucune borne chiffrée.

---

# Illustration : l'identifiant EST le hash du contenu

Snippet auto-portant (~50 lignes, dépendance : `sha2`). Montre que :
1. l'id d'une action **est** le SHA-256 de son contenu (+ ses parents) ;
2. changer un octet change l'id (tamper-evident) ;
3. un lien `caused_by` vers un id falsifié devient **orphelin**, détectable.

```bash
cd "$(git rev-parse --show-toplevel)/examples/blog-02-dag-causal/illustration"
cargo run            # affiche les ids, puis détecte la falsification (assertions)
```

La sortie (ids hex) est produite à l'exécution. Ce snippet n'utilise ni RocksDB, ni le
journal réel, ni la vérification complète du runtime : c'est une maquette du *principe*,
pas le système. La mesure et la vérification réelles sont dans `../REPRODUCE.md`.
