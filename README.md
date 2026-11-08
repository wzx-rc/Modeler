# Forme — modélisation hard surface sur iPad

Forme construit un objet en combinant des formes paramétriques au Pencil (Add, Carve, Intersect, Groove, Tongue), dans l'esprit du workflow « primitives » de Substance 3D Modeler, sans VR ni sculpt. Tout reste éditable : la scène est un arbre de formes, jamais un maillage.

Écrit entièrement en Rust : le même code tourne sur iPad (Metal) et sur ordinateur (Vulkan, DX12, GL) via wgpu.

## Architecture

| Crate | Rôle |
| --- | --- |
| `forme-core` | Arbre de formes, compilation en bytecode SDF, évaluation CPU, historique, projets JSON, maillage *dual contouring*, **Bake** (historique → maillage propre), export OBJ et GLB. Aucune dépendance plateforme. |
| `forme-render` | Viewport : un shader WGSL raymarche le bytecode (volumes englobants pour sauter les formes lointaines), surbrillance de la sélection et fantôme des formes de coupe ; aperçu du maillage baké (faces éclairées, filaire des vrais polygones). Capture hors écran. |
| `forme-app` | L'application : interface egui fidèle à la maquette, gestes (doigts = caméra, Pencil = outils), poignées, mode Profil, kit, export. Binaire `forme` (iPad et ordinateur) et `forme-shot` (captures automatiques). |

Le shader et l'interpréteur CPU exécutent le même bytecode : le picking, le maillage d'export et les tests utilisent le CPU, l'affichage le GPU.

## Gestes

| Geste | Action |
| --- | --- |
| 1 doigt | Orbite |
| 2 doigts | Pan + pinch zoom |
| Tap 2 doigts / 3 doigts | Annuler / Rétablir |
| Pencil sur une surface (outil forme) | Étire la base, relâche, puis glisse vers le haut pour la hauteur ; tap pour valider |
| Pouce maintenu sur « Carve » | La forme en cours de création creuse au lieu d'ajouter |
| Pencil, outil Profil | Tap = point d'angle, glisser = point de courbe, tap sur le premier point = fermer et extruder |
| Tap sur une forme | Sélection, poignées rayon / hauteur / bevel / déplacement |
| Double tap | Cadrer la sélection |

Sur ordinateur : clic gauche = Pencil, clic droit = orbite, molette = zoom, Maj + glisser = pan, Alt = Carve, Cmd/Ctrl+Z, Suppr, Cmd/Ctrl+D, F (cadrer), Entrée, Échap.

Aimantation : le Snap arrondit au demi-millimètre ; toucher le centre d'un bossage ou l'intérieur d'un perçage centre la nouvelle forme sur son axe.

## Bake : de l'historique paramétrique au maillage propre

Le bouton **Bake** convertit l'arbre de formes en maillage d'export sans passer par des voxels (`forme-core/src/bake`) :

1. **Termes** : le bytecode est réinterprété en double précision en suivant, pour chaque point, quelle copie de quelle primitive (et à quel niveau d'offset pour coques, Groove, Tongue) donne la surface.
2. **Surfaces paramétriques** : chaque primitive est maillée à partir de ses paramètres : anneaux de révolution (cylindre, cône, capsule, tore, tube, profil en révolution) avec disques tout en quads, boîtes arrondies, sphères en quads, extrusions arrondies (contours décalés et faces n-gones). Les copies des répétitions et symétries réutilisent le même maillage.
3. **Découpe exacte** : chaque copie n'est raffinée que là où une autre forme peut se cacher. Ses sommets sont projetés sur la surface finale (congés compris) et ses faces coupées par dichotomie le long des courbes où une autre forme prend le relais.
4. **Couture** : soudure des points de découpe, insertion des points de l'autre côté (pas de T-junctions), bandes trop fines comblées, réparation des arêtes non manifold.
5. **Normales** : normales exactes du champ, séparées sur les arêtes vives ; matières en couleurs de vertex.

Le résultat est figé : il n'a plus de lien avec le modèle, et toute modification demande un nouveau Bake. Le calcul se fait en arrière-plan sur tous les cœurs, avec progression et annulation. Trois précisions sont proposées (Brouillon, Standard, Max). L'export OBJ garde quads et n-gones, l'export GLB est triangulé.

Grenade remontée en précision Max : 290 000 faces, dont la moitié en quads, en ~24 s sur 2 cœurs. L'écart moyen à la surface exacte est de 0,002 mm. Il reste une vingtaine d'arêtes non manifold aux jonctions triples.

## Construire

```bash
cargo test -p forme-core                       # tests du noyau
cargo run -p forme-app --bin forme             # app sur ordinateur
cargo run -p forme-app --bin forme-shot -- shots   # captures de l'app réelle (GPU logiciel accepté)
```

### iPad (sans compte développeur)

Le workflow `.github/workflows/build.yml` compile `Forme.ipa` non signé sur un runner macOS (`ios/build_ipa.sh`) et le publie en artefact. Installe-le ensuite avec AltStore ou Sideloadly et ton Apple ID gratuit (à re-signer tous les 7 jours). Les projets `.forme` et les exports apparaissent dans l'app Fichiers, dossier « Forme ».

Sur un Mac : `bash ios/build_ipa.sh`.

## État

- J0 Socle : fait (noyau, shader, captures).
- J1 Arbre de formes : fait (9 primitives, 5 modes, blend lisse ou chanfrein, groupes fusionnés ou non, répétitions radiale et linéaire, symétries, undo/redo, projets).
- J2 UX tactile : fait (création au geste, poignées, snaps, pouce Carve, vues Face/Droite/Dessus orthographiques).
- J3 Hard surface : fait (profils extrudés ou en révolution, Groove/Tongue, kit vis/rivet/écrou/grille). La grenade de démo est reconstruite intégralement.
- J4 Export : fait (dual contouring à arêtes vives, GLB + OBJ, IDs matière en couleurs de vertex).
- J5 Bake : fait (maillage propre depuis l'historique, aperçu, export quads). Les répétitions dont les copies débordent de leur secteur sont désormais unies à la copie voisine : plus de couture, ni au rendu ni au Bake.

À faire ensuite : Bake sans aucune arête à reprendre aux jonctions triples, bandes de congés en rangées de quads, cache de briques du champ (performances au-delà de ~150 formes), réglage fin de la pression et du survol du Pencil, poignée de rotation, menu radial (squeeze Pencil Pro), rétopo par lignes de force à l'export.

Polices : Inter (SIL Open Font License, `crates/forme-app/assets/Inter-LICENSE.txt`).
