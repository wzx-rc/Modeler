# UX : gestes et interface

## iPad

| Geste | Action |
| --- | --- |
| 1 doigt (outil Sélection) | Orbite |
| 2 doigts | Pan + pinch zoom |
| Tap 2 doigts / 3 doigts | Annuler / Rétablir |
| Outil forme, **glisser** (doigt ou Pencil) | Base : boîte coin à coin, cylindre/sphère depuis le centre. Relâcher, puis glisser pour la hauteur ; tap pour valider (style 3ds Max) |
| Outil forme, tap | Forme de taille par défaut, hauteur encore réglable |
| Pencil après la base | La hauteur suit le survol ; tap pour valider |
| Après une création au doigt | Retour automatique en Sélection (1 doigt = orbite) |
| Pouce maintenu sur **Carve** | La forme en cours creuse au lieu d'ajouter |
| Pouce maintenu sur **CTRL** | Les taps ajoutent ou retirent de la sélection (arêtes) |
| Survol Pencil, outil Sélection | Met en évidence la jonction ou l'arête sous la pointe, avec étiquette (« + » ou « − » si CTRL) |
| Tap près d'une arête | Sélectionne cette arête seule : poignée Fillet, carte « Arête sélectionnée » (Arrondi / Chanfrein, rayon, Arête vive) |
| Tap sur une forme | Sélection, poignées (déplacement + équerres, rayon, hauteur, bevel) |
| Outil Profil (Pencil) | Tap = point d'angle, glisser = point de courbe, tap sur le premier point = fermer et extruder |
| Double tap | Cadrer la sélection |

## PC (clavier + souris)

- Clic gauche : comme le Pencil (créer, sélectionner, poignées). Le survol de la souris met en évidence les arêtes.
- Clic gauche glissé dans le vide (outil Sélection) : orbite. Clic molette ou Maj + glisser : pan. Molette : zoom.
- **Marking menu** (façon Maya / WZX Engine, 8 directions, `marking.rs`) : appui droit (geste rapide dans une direction = action directe, maintenu = menu affiché après 0,2 s), ou clic gauche maintenu immobile ~0,3 s. Sur une forme : Matière (arc de couleurs façon BrickIt), Symétrie X, Add / Carve, Cadrer, Supprimer, Masquer, Mode Arête, Dupliquer. Dans le vide : Cube, Cylindre, Snap, Grille, vues, Grouper. Sur une arête (mode Arête) : Fillet, Chanfrein, Arête vive, Cadrer, Mode Objet.
- Outils : Q sélection, R cube, C cylindre ; 1 / 2 : mode Objet / Arête. Options de création (petit bouton ••• sous les primitives) : interactive (base puis hauteur) ou directe (un clic pose la forme, les poignées la dimensionnent).
- Navigation façon Maya : Alt + clic gauche = orbite, Alt + clic molette = pan, Alt + clic droit glissé = zoom (avec n'importe quel outil). Ctrl / Cmd : ajouter à la sélection d'arêtes ; pendant la création, inverse + / −.
- Ctrl+Z / Ctrl+Y (ou Ctrl+Maj+Z) : annuler / rétablir. Ctrl+D : dupliquer.
- Suppr : supprimer la forme (sur une arête sélectionnée, retire son fillet). Échap : désélectionner, annuler. Entrée : valider. F : cadrer.

## Interface

- **Barre du haut** : nom du document, annuler/rétablir, vues 3D / Face / Droite / Dessus, Sym. X, Snap, Grille, Bake, Exporter.
- **Colonne d'outils à gauche** : Sélection, 9 primitives, Profil, Kit (vis à fente, rivet, écrou, grille), transformation.
- **Panneau droit** (276 pt) : propriétés de la sélection (forme, combinaison, blend lisse ou chanfrein, organisation radiale ou linéaire, symétrie, position et rotation), carte d'arête, arbre de formes.
- **Barre de modes en bas** : Add, Carve, Intersect, Groove, Tongue.
- **Boutons ronds en bas à gauche** : Carve (maintenir) et CTRL (maintenir).
- **Bake** : densité Légère / Standard / Dense, méthode affichée (par opération ou repli avec sa raison), aperçu du maillage (filaire), export OBJ (quads) et GLB.
- Sur iPad, l'interface est mise à l'échelle 0,84 (`UI_ZOOM`) : la maquette a été dessinée pour 1194 × 834 pt et paraissait trop grosse sur l'appareil.

## Viewport

- Raymarching du bytecode SDF : ombres, occlusion ambiante, grille, sélection en surbrillance, fantôme des formes de coupe.
- Rendu progressif : image rapide pendant que l'image change, image fine raffinée par bandes une fois immobile (l'UI n'est jamais bloquée).
- Budgets iPad : 3,8 Mpx à l'arrêt, 0,6 Mpx en mouvement, 0,3 Mpx raffinés par frame (adaptatif).
- Premier lancement : scène vierge. La grenade de démo est dans Documents.
