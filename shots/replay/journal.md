# Remontage de la grenade, scène vide → objet complet

**Étape 1 · Scène vide**

**Étape 2 · Fût : cylindre au sol, monté au gizmo, arrondi**
- rail : outil Cylindre
- Pencil : base aimantée sur grille (normale 0.00, 1.00, 0.00) → Cylindre 1 Add · taille 27.0 / 34.0 / 0.0 mm
- gizmo MoveY → position 0.0, 58.0, 0.0
- panneau : nom « Fût »
- panneau : Rondeur 10 mm, Taper 0,04
- panneau : Matière Vert militaire

**Étape 3 · Lobes : sphère au Pencil, radiale ×4, groupe ×2**
- rail : outil Sphère
- Pencil : base (normale -0.00, 0.02, 1.00) → Sphère 1 Add · taille 14.0 / 14.0 / 14.0 mm
- arbre : sélection de « Sphère 1 »
- panneau : Rayons 14 / 15,5 / 12 mm
- gizmo MoveZ → position -0.1, 46.0, 19.0
- panneau : Blend 5 mm, Vert, Répétition radiale ×4 (axe Y)
- arbre : Grouper
- panneau : groupe « Lobes », Répétition linéaire ×2, pas 24 mm en Y

**Étape 4 · Bossages : cylindre + vis du kit, radiale ×4, groupe ×2**
- rail : outil Cylindre
- Pencil : base (normale 0.10, 0.06, 0.99) → Cylindre 2 Add · taille 6.5 / 3.2 / 0.0 mm
- arbre : sélection de « Cylindre 2 »
- panneau : rotation 90° / 0 / 0 (axe exactement en Z)
- gizmo PlaneXY → position 0.0, 46.0, 37.2
- gizmo MoveZ → position 0.0, 46.0, 30.0
- panneau : nom « Bossage »
- panneau : Rondeur 1,2 mm
- panneau : Blend 1,5 mm, Vert
- arbre : Grouper (bossage + vis)
- arbre : sélection de « Bossage »
- rail : kit Vis à fente
- kit : vis à fente posée au centre du bossage
- arbre : sélection de « Groupe »
- panneau : Répétition radiale ×4
- arbre : Grouper
- panneau : groupe « Bossages », Répétition linéaire ×2, pas 24 mm en Y

**Étape 5 · Bague basse : anneau orange**
- rail : outil Cylindre
- Pencil : base aimantée sur grille (normale 0.00, 1.00, 0.00) → Cylindre 5 Add · taille 28.5 / 2.5 / 0.0 mm
- gizmo MoveY → position 0.0, 27.0, 0.0
- panneau : nom « Bague basse »
- panneau : Rondeur 1,2 mm
- panneau : Matière Orange

**Étape 6 · Base et pieds (équerre YZ)**
- rail : outil Cylindre
- Pencil : base aimantée sur grille (normale 0.00, 1.00, 0.00) → Cylindre 6 Add · taille 23.0 / 7.0 / 0.0 mm
- gizmo MoveY → position 0.0, 15.0, 0.0
- panneau : nom « Base »
- panneau : Rondeur 5 mm
- panneau : Blend 2 mm, Vert
- rail : outil Sphère
- Pencil : base (normale -0.01, -0.21, 0.98) → Sphère 2 Add · taille 6.0 / 6.0 / 6.0 mm
- arbre : sélection de « Sphère 2 »
- panneau : Rayons 6 / 4 / 6 mm
- gizmo PlaneYZ → position -0.2, 8.5, 15.0
- panneau : Blend 2 mm, Vert, Répétition radiale ×6

**Étape 7 · Col : noyau aimanté au centre, filetage ×5, collier**
- rail : outil Cylindre
- Pencil : base aimantée sur centre de la forme (normale 0.00, 1.00, 0.00) → Cylindre 7 Add · taille 14.0 / 9.0 / 0.0 mm
- arbre : sélection de « Cylindre 7 »
- panneau : nom « Noyau du col »
- panneau : Matière Métal sombre
- rail : outil Tore
- Pencil : base aimantée sur centre de la forme (normale 0.00, 1.00, 0.00) → Tore 1 Add · taille 14.5 / 0.0 / 1.5 mm
- gizmo MoveY → position 0.0, 93.5, 0.0
- panneau : « Filetage », Acier, Répétition linéaire ×5, pas 3,5 mm
- rail : outil Cylindre
- Pencil : base aimantée sur centre de la forme (normale 0.00, 1.00, 0.00) → Cylindre 8 Add · taille 19.0 / 4.0 / 0.0 mm
- arbre : sélection de « Cylindre 8 »
- panneau : nom « Collier »
- panneau : Rondeur 1,5 mm
- panneau : Matière Orange

**Étape 8 · Tête : bloc sur le collier, charnière sur sa face**
- rail : outil Boîte
- Pencil : base aimantée sur centre de la forme (normale 0.00, 1.00, 0.00) → Boîte 2 Add · taille 16.0 / 8.0 / 13.0 mm
- arbre : sélection de « Boîte 2 »
- panneau : nom « Bloc »
- panneau : Rondeur 3,5 mm
- panneau : Matière Métal sombre
- rail : outil Boîte
- Pencil : base (normale 1.00, 0.00, 0.00) → Boîte 3 Add · taille 7.0 / 8.0 / 8.5 mm
- arbre : sélection de « Boîte 3 »
- gizmo MoveX → position 13.0, 126.0, 0.0
- panneau : nom « Charnière »
- panneau : Rondeur 3 mm
- panneau : Blend 1 mm, Orange

**Étape 9 · Charnière : perçage en Carve, axe aimanté dans le trou**
- rail : outil Cylindre
- pouce : Carve maintenu
- Pencil : base (normale 0.94, -0.00, 0.34) → Cylindre 9 Carve · taille 2.5 / 10.0 / 0.0 mm
- arbre : sélection de « Cylindre 9 »
- panneau : « Trou d'axe », blend 0,3 mm, Métal sombre
- rail : outil Cylindre
- Pencil : base aimantée sur centre du perçage (normale 0.94, -0.00, 0.34) → Cylindre 10 Add · taille 1.5 / 9.5 / 0.0 mm
- arbre : sélection de « Cylindre 10 »
- gizmo MoveZ → position 24.2, 126.3, 0.0
- panneau : nom « Axe »
- panneau : Rondeur 0,4 mm
- panneau : Matière Acier

**Étape 10 · Tambour : posé sur la face gauche, placé à l'équerre XY**
- rail : outil Cylindre
- Pencil : base (normale -1.00, 0.00, 0.00) → Cylindre 11 Add · taille 12.0 / 19.0 / 0.0 mm
- arbre : sélection de « Cylindre 11 »
- gizmo PlaneXY → position -6.0, 142.0, 0.0
- panneau : nom « Tambour »
- panneau : Rondeur 2 mm
- panneau : Matière Métal sombre

**Étape 11 · Rainure en Groove, capuchon et bouchon aimantés**
- rail : outil Boîte
- barre de modes : Groove
- Pencil : base (normale 0.00, 1.00, 0.00) → Boîte 4 Groove · taille 0.5 / 20.0 / 20.0 mm
- arbre : sélection de « Boîte 4 »
- panneau : Largeur 0,2 mm (plan de coupe)
- panneau : « Rainure », profondeur 1,2 mm, largeur 0,9 mm, Métal sombre
- rail : outil Cylindre
- Pencil : base aimantée sur centre de la forme (normale -1.00, -0.00, 0.00) → Cylindre 12 Add · taille 10.5 / 3.0 / 0.0 mm
- arbre : sélection de « Cylindre 12 »
- gizmo MoveX → position -25.5, 142.0, 0.0
- panneau : nom « Capuchon »
- panneau : Rondeur 1,2 mm
- panneau : Matière Orange
- rail : outil Cylindre
- Pencil : base aimantée sur centre de la forme (normale -1.00, -0.00, 0.00) → Cylindre 13 Add · taille 6.0 / 3.5 / 0.0 mm
- arbre : sélection de « Cylindre 13 »
- gizmo MoveX → position -27.5, 142.0, 0.0
- panneau : nom « Bouchon »
- panneau : Rondeur 0,8 mm
- panneau : Matière Métal sombre

**Étape 12 · Levier : contour tracé au Pencil en vue Face**
- vue : Face (orthographique)
- rail : outil Profil
- Pencil : 21 points (taps = angles, glissés = courbes)

**Étape 13 · Goupille et anneau : grenade terminée**
- Pencil : tap sur le premier point → contour fermé, extrudé
- panneau : Épaisseur 9 mm, rondeur 1,2 mm
- panneau : « Lame », position Z −4 mm, Vert
- rail : outil Capsule
- Pencil : base (normale 0.99, 0.11, 0.00) → Capsule 1 Add · taille 1.5 / 6.0 / 0.0 mm
- arbre : sélection de « Capsule 1 »
- panneau : « Goupille », Acier
- rail : outil Tore
- Pencil : base aimantée sur grille (normale -0.00, -0.00, 1.00) → Tore 2 Add · taille 6.5 / 0.0 / 1.3 mm
- arbre : sélection de « Tore 2 »
- panneau : « Anneau », position Z −4 mm, Acier

23 formes dans l'arbre, 27 nœuds.

```
Fût
Lobes
  Lobe
Bossages
  Bossage + vis
    Bossage
    Vis
Bague basse
Base
Pieds
Noyau du col
Filetage
Collier
Bloc
Charnière
Trou d'axe
Axe
Tambour
Rainure
Capuchon
Bouchon
Lame
Goupille
Anneau
```