# Forme — modélisation hard surface sur iPad

Forme construit un objet en combinant des formes paramétriques au doigt et au Pencil (Add, Carve, Intersect, Groove, Tongue), dans l'esprit du workflow « primitives » de Substance 3D Modeler, sans VR ni sculpt. Tout reste éditable : la scène est un arbre de formes, jamais un maillage. Le **Bake** convertit l'historique en un maillage de quads propre, comme modélisé à la main.

Écrit entièrement en Rust : le même code tourne sur iPad (Metal) et sur ordinateur (DX12, Vulkan) via wgpu.

**Pour reprendre le projet** : `CLAUDE.md` (règles et commandes), puis `docs/00-PROJET.md` (vision, décisions, état, suite).

| Doc | Contenu |
| --- | --- |
| [`docs/00-PROJET.md`](docs/00-PROJET.md) | Dossier de projet (à déposer dans un Projet Claude) |
| [`docs/INSTRUCTIONS-PROJET.md`](docs/INSTRUCTIONS-PROJET.md) | Instructions à coller dans le Projet Claude |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Carte du code |
| [`docs/BAKE.md`](docs/BAKE.md) | Bake par opération : exigences, algorithme, limites |
| [`docs/UX.md`](docs/UX.md) | Gestes iPad et PC, interface |
| [`docs/DEV-PC.md`](docs/DEV-PC.md) | Installation Windows, commandes, `.ipa` |
| [`docs/HISTORIQUE.md`](docs/HISTORIQUE.md) | Chronologie et retours de Romain |
| `docs/images/` | Captures clés |

## Démarrer

```bash
cargo run --release -p forme-app --bin forme                   # l'app sur ordinateur
cargo test --release -p forme-core                             # tests du noyau
cargo run --release -p forme-app --bin forme-shot -- shots     # captures de l'app réelle
```

Le `.ipa` pour iPad est compilé par GitHub Actions à chaque push sur `main` et publié dans la release `ipa-latest` (voir `docs/DEV-PC.md`).

## Crates

| Crate | Rôle |
| --- | --- |
| `forme-core` | Arbre de formes, bytecode SDF et interpréteur CPU, historique, projets JSON, export OBJ/GLB, sélection d'arêtes (`edges.rs`), **Bake** (`bake/`). Aucune dépendance plateforme. |
| `forme-render` | Raymarcher WGSL du bytecode, aperçu du maillage baké, capture hors écran. |
| `forme-app` | Interface egui, gestes, outils, rendu progressif, binaires `forme` (iPad et PC) et `forme-shot` (captures automatiques). |

## État (9 novembre 2026)

- **Modélisation** : 9 primitives, 5 modes, blend lisse ou chanfrein, groupes, répétitions, symétries, profils, kit (vis, rivet, écrou, grille), undo/redo, projets.
- **Fillets** : par arête sur la boîte et le cylindre, par blend sur les jonctions. Sélection d'arêtes au survol ou au tap, avec CTRL pour en ajouter.
- **Création façon 3ds Max** : base glissée, puis hauteur ; au doigt comme au Pencil.
- **Bake par opération** : tout l'historique en chaîne. Cube test : 100 % quads. Scènes avec pièces du kit : plus de 90 % quads, fermé. Repli expliqué sur l'ancien bake sinon.
- **iPad** : rendu progressif, budgets de pixels, UI à 84 %. Dernières corrections pas encore validées sur l'appareil.

Prochaines étapes : voir `docs/00-PROJET.md` §5.

Polices : Inter (SIL Open Font License, `crates/forme-app/assets/Inter-LICENSE.txt`).
