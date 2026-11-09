# Historique du développement (novembre 2026)

1. **Plan et maquette** : Romain les a validés. Il a demandé le développement complet en Rust pour iPad Pro, avec des captures de suivi.
2. **J0 à J4** :
   - noyau SDF, raymarcher, interface egui fidèle à la maquette ;
   - gestes tactiles, poignées, snap, profils, Groove/Tongue, kit de pièces ;
   - export par dual contouring (GLB, OBJ).
3. **Grenade sci-fi** : reconstruite de zéro avec les seuls outils de l'app (13 étapes rejouées, timelapse).
4. **Équerres** sur le gizmo de déplacement (déplacement dans un plan).
5. **Bake v1 (par surfaces)** : patchs découpés et cousus. Sur la grenade : 290 000 faces, très précis, mais **topologie refusée** (« on dirait du voxel converti »).
6. **Référence de Romain** : cylindre Blender avec panneau en retrait, et fillets « comme un bevel modifier ou une conversion NURBS ». Son idée : chaque outil mémorise ses valeurs et son tracé.
7. **Dessin cible** d'un cube avec bevel et trou cylindrique, validé avant d'écrire le code (« pour voir si on est d'accord »).
8. **Remaillage orienté champ** : essayé puis abandonné (topologie irrégulière).
9. **Bake par opération** sur le cube test : la topologie de la cible est reproduite par le calcul (1408 quads). « Bevel plus sharp » : passé à 1 mm.
10. **Sélection d'arêtes** en mode Select : survol, tap, fillet par poignée.
11. **Une arête à la fois + bouton CTRL** (maintenir pour ajouter). Fillets par arête dans le shader, le CPU et le bake.
12. **Dépôt GitHub `wzx-rc/Modeler`** et `.ipa` par GitHub Actions.
13. **Premier test sur iPad** : ça ramait énormément. Corrections :
    - rendu progressif (l'UI ne dépend plus du raymarching) ;
    - budgets de pixels ;
    - démarrage sur une scène vierge.
14. **Deuxième test sur iPad** : fluide, mais trois retours. Corrections :
    - qualité : l'app restait sur l'image rapide en mode placement ;
    - UI trop grosse : passée à 84 %, panneau plus étroit ;
    - création façon 3ds Max au doigt ;
    - bake de sa scène (boîte, profil, 4 vis) passé à l'ancien bake : bake par opération étendu à tout l'historique en chaîne.
15. **Paquet de transfert** vers un Projet Claude et un développement sur PC (ce dossier `docs/`, `CLAUDE.md`).
