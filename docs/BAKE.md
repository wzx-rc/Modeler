# Bake : de l'historique paramétrique à un maillage propre

## Ce que Romain attend

- Un maillage d'export **comme modélisé à la main** : grille de quads régulière sur les grandes faces, et des boucles qui suivent les formes (un trou dans une face : cercle dans un carré, avec des anneaux de raccord).
- Des fillets et bevels en **rangées de quads**, comme un bevel modifier de Blender ou une conversion NURBS.
- Surtout pas l'aspect « voxel converti en poly ».
- Ponctuel : pas de lien dynamique, Bake complet après chaque modification, aussi précis que nécessaire.
- Valider sur des cas simples (cube test), puis étendre.

Cible dessinée à la main et validée : `docs/images/16-cible-cube-trou.jpg`. Cube de 20 mm, bevel 1 mm en 4 rangées, grille 8 × 8, trou Ø8 en 32 segments avec 3 anneaux de raccord, fillet de 0,8 mm en 2 rangées, parois en rangées.

## Ordre de construction (règle validée par Romain, 9 nov. 2026)

Toujours dans cet ordre, à garder pour toutes les jonctions :

1. **D'abord les fillets.** Chaque congé ou arrondi (jonction entre deux formes, arrondi d'une forme, congé d'un trou) devient une bande de quads, construite depuis ses rails et son arête vive : une colonne = une section en biseau perpendiculaire à l'arête vive, des rangées comme un bevel modifier.
2. **Les matérialiser en orange** dans les rendus de mise au point, pour voir les bandes seules avant le reste.
3. **Ensuite les paths de remplissage** : les faces plates et les parois se remplissent entre les bords des bandes, en reprenant exactement leurs sommets (soudés, jamais recousus après coup). Si une bande rejoint une face déjà posée, ses colonnes partent des sommets de cette face.

Le Bake par lignes (`layout.rs`) applique cette règle : bandes de fillet d'abord (rôle orange, `P_FILLET`), puis les patchs de remplissage (rôle crème, `P_FILL`) qui reprennent leurs sommets.

### Passe secondaire : retirer les loops plates (9 nov. 2026, option, désactivée par défaut)

Validée par Romain, mais **jamais appliquée par le Bake par défaut**. C'est une option de la fenêtre Bake (« Passe secondaire · Retirer les loops plates (< 5°) ») qui tourne en arrière-plan sur le résultat du Bake ; la désactiver rend le Bake sans la passe, et l'export prend le maillage affiché. Code : `bake/decimate.rs` (`dissolve_flat_loops`, seuil `FLAT_LOOPS_DEG` = 5°), par tours : toutes les loops candidates, la plus plate d'abord, celles qui partagent une face avec une loop déjà retirée attendent le tour suivant.

Sur les bakes de l'app : cube 20 mm 384 → 6 faces, cube arrondi 864 → 150, cube test (trou + fillet) 1376 → 624, cylindre arrondi 1392 → 1056, grenade (ancien bake par surfaces) 132 345 → 125 737 en 3,3 s (chiffres du 9 nov., avec les anciens bakes). Toujours fermé, aucun défaut ajouté.

`bake_probe decime` (seuil `DEG`, 5° par défaut) appelle la même fonction sur le bake de la scène de Romain et retire les edge loops fermées qui ne font pas de volume : en chaque sommet de la loop, le chemin qui la traverse (voisin d'un côté, sommet, voisin de l'autre) ne sort pas de la surface de plus de `DEG`. Les deux quads de part et d'autre fusionnent. Scène de Romain : 15 loops, 390 → 44 quads, fermé.

Limite : une loop n'est retirée qu'entière. Celles qui passent par un trou, un éventail de fillet ou un pôle (coin du carré d'un trou) restent, même sur leurs morceaux plats (dessous du socle).

## Le Bake : par lignes (`layout.rs`), seul algorithme depuis le 10 nov. 2026

Demande de Romain : le maillage se déduit des **lignes de construction** (arêtes des formes, rails de leurs arrondis, ligne de jonction entre deux formes), en commençant par la ligne de contour entre deux formes.

**Un seul algorithme, pas de repli** (Romain, 10 nov.) : `bake()` appelle seulement `layout::build`. Un historique qu'il ne gère pas encore renvoie `Err(raison)` : pas de maillage, la raison s'affiche en rouge dans le panneau Bake (et avant de lancer, « Pas encore géré : … » d'après `bake::plan`). Les anciens bakes (par opération `build.rs`, par surfaces `patch.rs`, orienté champ `quad.rs`) sont supprimés.

1. **Cartes** : chaque face de chaque boîte (face plate = rectangle dans son plan, arête arrondie = bande « le long de l'arête × le long de l'arc ») est découpée par les autres formes. Les bords de la région sont tracés en 2D puis posés exactement sur la face voisine ; les coins sont résolus à l'intersection exacte des deux faces. Chaque bord est partagé par les deux faces qu'il sépare (mêmes sommets des deux côtés).
2. **Fillets d'abord** : le nombre de rangées à travers chaque bande est fixé (tolérance de corde, pair), rôle orange.
3. **Boucles** : depuis chaque coin rentrant (vif, ou le milieu d'un fillet concave) des lignes partent tout droit à travers la face et continuent de face en face jusqu'à se refermer. Pour un fillet convexe (coin arrondi d'une face), ses rails continuent en boucles et son milieu devient le coin du patch arrondi.
   **Accroche en diagonale** (par défaut, validée par Romain le 10 nov.) : le milieu d'un arc concave (fillet ou cylindre) part en diagonale vers un nœud de la grille, à une case du coin formé par les tangentes de l'arc à ses deux bouts ; les boucles de la face partent de ce nœud. Les rangées de l'arc s'ouvrent en éventail dans les deux patchs du coin au lieu de traverser toute la pièce en bandes fines.
   **Cylindres** (vifs) : la paroi est une carte « le long de l'axe × autour » dont la couture est placée au milieu du plus grand secteur caché ; le nombre de segments autour suit la tolérance de corde. Un cylindre visible sur tout le tour (trou rond) est coupé en deux moitiés le long de son axe x local, chacune une carte ; sur la face percée, le cercle est coupé en quatre quarts, chacun accroché en diagonale : c'est le motif « cercle dans un carré », dont les côtés continuent en boucles sur la pièce.
4. **Patchs** : chaque face est alors coupée en patchs à 4 coins. Les nombres de segments sont accordés (côtés opposés égaux), l'espacement des lignes libres reprend celui de la ligne d'en face (les rangées du fillet continuent droites), puis chaque patch est rempli par interpolation transfinie dans sa carte.

Résultats : scène de Romain (boîte + coin creusé, fillet 2,2 mm) 390 quads en diagonale (432 en rangées droites), fermé, écart max 0,04 mm ; même scène sans fillet 318 quads ; boîte à une arête arrondie 450 quads ; cube avec un cylindre creusé dans un coin 482 quads, écart max 0,04 mm ; plaque percée d'un trou de Ø 12 : 480 quads, écart max 0,05 mm (`bake_probe parlignes`, `CASE=coin` pour un seul cas). Pour l'instant : boîtes et cylindres vifs (trous compris), Add / Carve, fillet de jonction d'une boîte posée sur une face. Le reste renvoie `Err` : la raison s'affiche en rouge dans le panneau Bake, sans maillage.

### Règles ajoutées le 10 nov. (scène de Romain à fillets par arête)

- **Coin en boule** : où trois arêtes arrondies du même rayon se rejoignent, le coin est un huitième de sphère (carte gnomonique `Map::Sph`), coupé en trois quads autour de son centre ; les bandes s'arrêtent à la boule. Trois rayons différents : pas encore géré (raison affichée).
- **Coin arrondi d'une face = un patch** : une boucle qui arrive de la face sur le carré du coin s'arrête sur son côté (un T), au lieu de couper un triangle contre l'arc. Les rangées de la bande qui entrent par l'arc traversent le coin parallèlement au rail et s'arrêtent en T de l'autre côté. Une boucle déjà tracée qui passe par un T est coupée en ce point.
- **Tangentes exactes** aux bouts des lignes de jonction (gradient de la surface d'en face), pas la direction de la polyligne : sinon les boucles dérivent d'un degré et ratent les nœuds.
- **Accroche proche** : une boucle qui arrive à moins d'environ 1° d'un nœud existant le prend, au lieu de laisser une rangée très fine.
- **Trou rond borgne / disque vu en entier** (fond d'un trou qui ne traverse pas, dessus d'un cylindre) : o-grid. Un carré au milieu (coins à 0,6 r) dont les coins font face aux milieux des quatre quarts d'arc, là où arrivent les lignes du mur. Rien ne traverse le disque (scène `romain-cylindre.forme` sans raccord : 724 quads).
- **Zones réservées** : le carré entre le centre d'un arc concave accroché en diagonale et son nœud d'accroche (l'anneau d'un trou rond, le coin d'un congé concave), et le carré du milieu d'un disque. Une boucle venue d'ailleurs qui y entrerait s'arrête en T sur le côté par où elle entre, si ce côté est une ligne.

- **Fillet de jonction (raccord, blend d'un Add)** : pour l'instant une boîte à arêtes vives posée d'aplomb sur une face plane. Le profil est exact (smin quadratique entre deux plans : √u + √v = √k, longueur ≈ 1,62 k). Quatre bandes le long des parois (cartes `Map::Blend`, rangées comme un fillet), et à chaque coin de la boîte un quart de révolution autour du pied de l'arête (`Map::Rev`), coupé en trois quads autour d'un nœud, comme le coin en boule. Le pied de l'arête vive reste une pointe (c'est la forme du smin, pas un défaut du maillage). La face autour reçoit une accroche en diagonale au milieu de chaque coin, et ces accroches se recalent sur les boucles déjà prévues (anneau d'un trou voisin) ou partagent un nœud proche. Scène `romain-cylindre.forme` avec le raccord de 3,1 mm : 794 quads, fermée, écart max 0,09 mm. Pas encore : blend sur un Carve, boîte arrondie ou penchée, autre forme, raccord trop près d'une arête ou d'une autre forme (`Err`).

### Encore à faire (renvoie `Err` aujourd'hui)

- Fillets de jonction autres que la boîte posée sur une face (blend d'un Carve, de formes arrondies ou penchées), chanfreins, Intersect, Groove, Tongue.
- Cylindres arrondis, sphères, profils, tores, coques, taper.
- Copies (répétitions, symétries), groupes fusionnés (historique en arbre).
- Coin où trois fillets de rayons différents se rejoignent ; coin arrondi coupé par une autre forme.
- Cube test et grenade : en attente de ces cas.

## Historique des approches (supprimées le 10 nov. 2026)

1. **Bake par surfaces** (`patch.rs`). Patchs paramétriques découpés et cousus. Précis, mais topologie refusée par Romain (« voxel converti »).
2. **Remaillage orienté champ** (`quad.rs`, style Instant Meshes). Fillets irréguliers, non-manifold. Abandonné.
3. **Bake par opération** (`build.rs`). Chaque opération refaisait une région autour de ses rails (anneaux de raccord, lofts). Cube test 1408 quads ; remplacé par le Bake par lignes, plus proche de ce que Romain veut (boucles qui suivent les lignes de construction).

Le code est dans l'historique git (commit avant le nettoyage du 10 nov.) si une idée doit être reprise.

## Outils de mise au point

- `OUT=/tmp/x cargo run --release -p forme-core --example bake_probe -- parlignes` : le Bake sur ses scènes de test, rendus CPU (`CASE=romain-aretes` pour une seule, `DEBUG_LAYOUT=1` pour les rayons tracés et les patchs refusés). Bandes de fillet en orange, remplissage en bleu.
- `… bake_probe -- lignes` : lignes de construction (`lines.rs`) sur la pièce bakée. `… bake_probe -- decime` : la passe optionnelle.
- `forme-shot -- /tmp/x 19` (cube test dans l'app : montre le message « pas encore géré »), `22` (fillets par arête, bake), `user` (scène de test de Romain).
