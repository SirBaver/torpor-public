<!-- Corps de la GitHub Release `blog-08-la-methode` (releases jumelles FR/EN). Article méta : la « preuve » est le corpus versionné (ADR de réfutation). Pas de commande de reproduction propre. -->

# blog-08 — Trois fois, nos données ont contredit notre modèle

**Régime :** méta (méthode) · **Statut :** corpus public et versionné.

## Claim → Preuve

| Claim de l'article | Preuve (permalink `@blog-08-la-methode`) | Substrat |
|---|---|---|
| 3 ADR de réfutation (données vs modèle) | ⟨decisions/0032⟩, ⟨0033⟩, ⟨0034⟩ | Linux |
| Hypothèse de fuite mémoire réfutée (RSS bornée ~793 Mo @500) | ⟨decisions/0034-refutation-fuite-memoire-t6-soak.md⟩ | Linux/RocksDB |
| Critère de mesure inapplicable (R²=0,24) | ⟨decisions/0033-critere-fuite-memoire-lsm.md⟩ | Linux |
| 65 décisions versionnées | ⟨decisions/INDEX.md⟩ | — |
| Refus de mesurer sur mauvais substrat (intégrité) | ⟨decisions/0065⟩, ⟨0049⟩ | — |

## Limites
- Cette méthode ne « prouve pas qu'on a raison » : elle rend les erreurs **visibles et révisables**. Elle ne transforme pas un prototype de recherche en produit.
- Tous les chiffres cités sont **substrat Linux/RocksDB**, non transférés à la cible seL4 (ADR-0065).
