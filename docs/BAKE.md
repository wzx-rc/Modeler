# Bake : de l'historique paramétrique à un maillage propre

## Ce que Romain attend

- Un maillage d'export **comme modélisé à la main** : grille de quads régulière sur les grandes faces, et des boucles qui suivent les formes (un trou dans une face : cercle dans un carré, avec des anneaux de raccord).
- Des fillets et bevels en **rangées de quads**, comme un bevel modifier de Blender ou une conversion NURBS.
- Surtout pas l'aspect « voxel converti en poly ».
- Ponctuel : pas de lien dynamique, Bake complet après chaque modification, aussi précis que nécessaire.
- Valider sur des cas simples (cube test), puis étendre.

Cible dessinée à la main et validée : `docs/images/16-cible-cube-trou.jpg`. Cube de 20 mm, bevel 1 mm en 4 rangées, grille 8 × 8, trou Ø8 en 32 segments avec 3 anneaux de raccord, fillet de 0,8 mm en 2 rangées, parois en rangées.

## Historique des approches

1. **Bake par surfaces** (`patch.rs`, encore utilisé en repli). Patchs paramétriques par primitive, découpés exactement le long des courbes de jonction puis cousus. C'est précis (écart moyen 0,002 mm), mais la topologie est mauvaise : raffinements adaptatifs, triangles. **Refusé par Romain.**
2. **Remaillage orienté champ** (`quad.rs`, style Instant Meshes). Grille uniforme correcte sur les faces, mais les fillets sont irréguliers, il y a des non-manifold, et aucune garantie du motif « cercle dans un carré ». **Abandonné.**
3. **Bake par opération** (`build.rs`, actuel). C'est l'idée de Romain : chaque opération construit sa topologie à partir de ses paramètres.

## Bake par opération (`build.rs`)

**Entrée**
- `Field::steps()` lit l'historique à plat : une forme de base, puis une forme combinée à la fois (Add ou Carve, blend k).
- Les groupes transparents (« Fusionner les enfants » désactivé) donnent une chaîne à plat.
- Chaque copie d'une forme (répétitions, symétries) est une sous-étape.

**Étape 0 : maillage natif de la forme de base** (`gen.rs`). Pour une boîte arrondie : grille par face, rangées de bevel, coins en patchs. Fillets par arête : bandes par face aussi larges que le plus grand fillet autour de la face, puis chaque sommet est projeté depuis le point intérieur de sa bande (étanche par construction).

**Étape i** (`apply_step`), avec `pre = field.prefix(i)` (le modèle avant), `post = prefix(i+1)`, et l'outil (primitive + copie) :

1. **Rails exacts.** Le fillet de blend k entre la surface A (avant) et l'outil B a deux rails :
   - sur A : `{sa = 0, sb = k}` ;
   - sur B : `{sb = 0, sa = ∓k}` (Carve : −k, Add : +k) ;
   - plus le coin `{sa = 0, sb = 0}`.

   On trace un contour de `sb − k` sur le maillage courant, puis chaque point est affiné par **Newton à deux contraintes** (pas de norme minimale, `solve2`) et densifié.
2. **Région à refaire sur A.**
   - Les faces dans le rail sont forcées.
   - S'y ajoutent les faces à moins de `dégagement` du rail : une fraction (0,6) de la taille de la pièce, au plus une maille.
   - Ces faces doivent rester proches du plan tangent du rail (pas de raccord par-dessus un bevel).
   - Les faces faites par des étapes précédentes ne sont reprises que si le rail les traverse.
   - La région est complétée jusqu'à ne plus avoir d'encoche (contour propre) et doit être un disque à un seul contour.
3. **Échantillonnage du rail** : autant de points que de sommets sur le contour de la région, aux mêmes fractions d'abscisse curviligne, avec la phase et le sens qui collent au contour.
4. **Fillet en rangées** : pour chaque colonne, un rail A → coin → rail B (Bézier quadratique), projeté sur le champ final. Le nombre de rangées dépend de la longueur du profil et de la largeur des colonnes (au moins 2).
5. **Anneaux de raccord sur A** : du rail au contour de la région, avec une hauteur graduée de la taille des colonnes près du rail jusqu'à la maille native. Chaque point est projeté sur A.
6. **Outil B** :
   - maillage natif avec autant de colonnes que le rail quand c'est une révolution (`GenSettings.segs`) ;
   - on garde les faces bien à l'intérieur de A (Carve) ou à l'extérieur (Add), et on raccorde au rail B par des anneaux ;
   - si les nombres de sommets diffèrent : anneaux à **compte variable** et quads « 3 + 1 » (`loft_varying`, `bridge_quads`). Il ne reste qu'un triangle si la différence est impaire.
7. **Orientation** propagée par les arêtes partagées, puis signe global voté sur le gradient du champ.

**Fin** : `finish()` calcule les normales exactes du champ et les statistiques (quads, triangles, bords ouverts, non-manifold, écart à la surface).

**Repli** : toute situation non gérée renvoie `Err(raison)`. `bake()` utilise alors l'ancien bake et la fenêtre Bake affiche la raison en jaune, avec la forme concernée.

## Résultats et limites (9 nov. 2026)

| Cas | Résultat |
| --- | --- |
| Cube test | 1408 faces, 100 % quads, 0 bord ouvert, 0 non-manifold, écart max 0,044 mm, ~15 ms |
| Variantes (trou décalé, sur le côté, borgne, bossage, arête vive, près du bord, fillets par arête) | Toutes 100 % quads et fermées |
| Boîte + 4 vis du kit | ~2100 faces, 93 % quads, fermé |
| Vis qui se chevauchent | Quelques non-manifold |

**Encore à faire**
- Plusieurs trous dans une même région (région à plusieurs contours).
- Jonction d'un profil extrudé avec la boîte.
- Intersect, chanfrein de jonction, coques.
- Groupes fusionnés (historique en arbre).
- Copies de la forme de base.
- Grenade.

## Outils de mise au point

- `cargo run --release -p forme-core --example bake_probe -- build | variants | screws | target` : rendus CPU filaires dans `/tmp`.
- `BY_STEP=1` : couleur par rôle (natif, anneaux, fillet, raccord paroi, paroi native).
- `ROUND=…` : bevel du cube test. `SIZE=…` : taille des quads.
- `forme-shot -- /tmp/x 19` (cube dans l'app), `22` (sélection d'arêtes, fillets par arête, bake), `user` (scène de test de Romain).
