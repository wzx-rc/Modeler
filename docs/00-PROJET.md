# Forme — dossier de projet

*État au 9 novembre 2026. Ce fichier est fait pour être déposé tel quel dans la base de connaissances d'un Projet Claude.*

## 1. Vision (validée avec Romain)

- App de **modélisation hard surface sur iPad Pro**, proche de l'UI/UX de **Substance 3D Modeler** mais sans VR. Romain avait donné cette vidéo de référence : https://www.youtube.com/watch?v=_Ee2sltrUKk
- On modélise au doigt et au Pencil en ajoutant ou soustrayant des formes de façon fluide. C'est proche de la **CAO**, pas du sculpt (pas un ZBrush).
- Écrit en **Rust** pour les performances : egui pour l'interface, wgpu pour le rendu (Metal sur iPad, DX12 ou Vulkan sur PC).
- Chaque forme, bevel et fillet reste **paramétrique** : la scène est un arbre de formes SDF, jamais un maillage.
- Le **Bake** convertit l'historique en maillage d'export propre, **en quads, comme un maillage modélisé à la main**. Il est ponctuel : aucun lien dynamique, on refait un Bake complet après chaque modification, et il peut prendre tout le temps et toute la puissance nécessaires.

## 2. Décisions et retours de Romain (à respecter)

| Sujet | Décision |
| --- | --- |
| Topologie du Bake | Le premier bake (surfaces découpées) a été refusé : « on dirait qu'on convertit du voxel vers poly ». Romain veut une grille de quads régulière et des boucles qui suivent les formes. Sa référence : un cylindre Blender avec un panneau en retrait. Les fillets et bevels doivent ressembler à un **bevel modifier** ou à une conversion **NURBS**, en rangées de quads. |
| Méthode | C'est son idée : chaque outil mémorise ses valeurs et son tracé exact, et on génère des chemins propres à partir de là. Aujourd'hui c'est le **Bake par lignes de construction**, seul algorithme depuis le 10 nov. (`docs/BAKE.md`). |
| Avancer petit à petit | D'abord le **cube test** (20 mm, bevel 1 mm, trou traversant Ø8, fillet 0,8). Romain a validé un dessin cible de la topologie avant le code (`docs/images/16-cible-cube-trou.jpg`). Ensuite seulement les scènes complexes (la grenade). |
| Bevel du cube | « Plus sharp » : 1 mm au lieu de 2. |
| Sélection d'arêtes | En mode Select, le survol met en évidence une jonction ou une arête ; un tap la sélectionne. **Une seule arête à la fois.** Un gros bouton **CTRL** en HUD à gauche, maintenu, ajoute ou retire des arêtes de la sélection. Le fillet se règle avec une poignée ou un curseur. |
| Création des primitives | **Style 3ds Max** : glisser la base (la boîte se trace coin à coin), relâcher, puis glisser la hauteur (au Pencil, la hauteur suit le survol) et taper pour valider. |
| iPad | Démarrer sur une **scène vierge**. L'UI était trop grosse, surtout le panneau de droite : elle est maintenant à 84 %, avec un panneau de 276 pt. Le viewport doit rester fluide. |
| PC d'abord (9 nov. 2026) | Pause du développement iPad direct. On construit les algorithmes et l'UI/UX sur PC, en Rust, et on revient à l'iPad ensuite (même code). Ressenti Substance Modeler / Spline / Plasticity, avec un côté pro 3ds Max. |
| Deux interfaces (9 nov. 2026) | Les deux maquettes ont été retenues. Dans **Réglages** (roue dentée), on bascule entre **Atelier** (viewport plein écran, panneaux flottants, historique en bas) et **Studio** (menus, explorateur, panneau de commandes Créer / Modifier / Affichage, barre d'état, façon 3ds Max). Les deux partagent la scène, les outils et le rendu ; l'UX est totalement différente. Le choix est gardé dans `settings.json`. |
| Fonctions de base | Par défaut, seulement **Cube et Cylindre** et **Add / Carve / Intersect**. Le reste (autres primitives, Groove, Tongue, profil, kit, répétitions, taper, coque) est caché, pas supprimé : interrupteur « Toutes les primitives et tous les modes » dans Réglages. |
| Distribution | Pas de compte développeur Apple. Le `.ipa` est compilé par GitHub Actions (dépôt `wzx-rc/Modeler`), puis installé avec Sideloadly ou AltStore. |

## 3. Ce qui existe (fonctionnel)

**Modélisation**
- 9 primitives : boîte, cylindre, sphère, capsule, tore, cône, prisme, tube, profil.
- 5 modes : Add, Carve, Intersect, Groove, Tongue.
- Blend lisse ou chanfrein.
- Groupes (fusionnés ou transparents), répétitions radiale et linéaire, symétries X/Y/Z.
- Undo/redo et projets `.forme` en JSON.
- Profils dessinés au Pencil, extrudés ou en révolution.
- Kit de pièces : vis à fente, rivet, écrou hexagonal, grille d'aération.

**Interface**
- egui, fidèle à la maquette validée.
- Poignées de déplacement, avec équerres (déplacement dans un plan), rayon, hauteur et bevel.
- Snap.
- Vues 3D, Face, Droite et Dessus (orthographiques).
- Bouton pouce « Carve » (maintenir) et bouton **CTRL** (maintenir).

**Fillets par arête**
- Chaque arête de boîte (12) et chaque bord de cylindre (2) peut avoir son propre arrondi ou chanfrein, évalué dans le shader, sur le CPU et dans le bake.
- Les jonctions entre formes se règlent par le blend de l'opération.

**Sélection d'arêtes**
- Survol, tap et CTRL pour ajouter (`forme-core/src/edges.rs`).
- Les courbes sont exactes ; les parties cachées sont dessinées en filigrane.

**Rendu**
- Raymarching du bytecode SDF.
- Rendu progressif : image rapide en mouvement (résolution adaptative), image fine raffinée par bandes une fois immobile.
- Budgets en pixels sur iPad.

**Export**
- OBJ (quads et n-gones conservés) et GLB.
- Maillage voxel par dual contouring (ancien export) ou maillage baké.

**Bake**
- Un seul algorithme : le Bake par lignes de construction (boîtes à arêtes vives ou arrondies, cylindres vifs, Add / Carve vifs).
- Pas de repli : un historique pas encore géré donne un message en rouge dans le panneau Bake, sans maillage.

**Captures automatiques** : `forme-shot`, avec des scénarios pour chaque fonctionnalité.

**iPad**
- Testé par Romain sur iPad Pro, avec une version de l'app antérieure aux derniers changements.
- Fluide après le rendu progressif.
- La dernière version (qualité du viewport, UI resserrée, création 3ds Max, bake en chaîne) n'a pas encore été vue sur l'appareil.

## 4. Chiffres de référence

| Cas | Résultat |
| --- | --- |
| Scène de Romain (boîte + coin creusé, fillet 2,2 mm), Bake par lignes | 390 quads, 100 % quads, fermé, écart max 0,04 mm, ~50 ms |
| Plaque percée d'un trou Ø12 | 480 quads, fermé |
| Cube test, grenade | pas encore gérés par le Bake par lignes (fillet de jonction, sphères…) |

## 5. Prochaines étapes (ordre proposé)

1. PC : retour de Romain sur les deux interfaces (Atelier / Studio), puis les 4 vues de Studio (Dessus, Face, Gauche, Perspective : rendu multi-caméras dans `gfx.rs`). L'iPad attend.
2. Bake par lignes, cas par cas : fillets de jonction, cylindres arrondis, chanfreins, Intersect, copies, groupes fusionnés (liste dans `docs/BAKE.md`).
3. Cube test puis grenade avec le Bake par lignes.
4. Fillets par arête pour prisme et cône. Raccord de coin lisse quand des rayons différents se rencontrent (aujourd'hui une petite cassure).
5. Performances au-delà de ~150 formes (cache de briques du champ).
6. Pencil Pro : squeeze pour un menu radial, survol plus fin.

## 6. Où trouver quoi

- Code et règles : `CLAUDE.md` à la racine, puis `docs/ARCHITECTURE.md`.
- Bake : `docs/BAKE.md`.
- Gestes et UI : `docs/UX.md`.
- Mettre en place le PC, compiler, iPad : `docs/DEV-PC.md`.
- Chronologie des sessions : `docs/HISTORIQUE.md`.
- Captures clés : `docs/images/`.
