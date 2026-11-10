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

- **Raccord sous un cylindre debout** (blend d'un cylindre posé sur une face plane) : le même profil tourné autour de l'axe, un anneau de rangées (`Map::Ring`), en deux moitiés comme le mur du cylindre (mêmes coutures). La face autour le voit comme un trou rond : quatre quarts accrochés en diagonale. Cas simple (bossage Ø 6 sur un socle, raccord 2 mm) : 296 quads, fermé. Pas encore : cylindre caché en partie, couché, arrondi.
- **Pas la place d'une accroche : diagonales jusqu'au bord** (option A choisie par Romain le 10 nov.) : quand le raccord ou le trou arrive à moins d'environ 0,6 quad d'un bord ou d'une autre forme (scène `romain-bossage.forme` : 1,7 mm du bord avant pour des quads de 5 mm), la diagonale ne s'arrête pas à une case du coin : elle va jusqu'au bord en face, et le carré d'accroche s'appuie sur ce bord. Jamais de rangée fine.
  - La diagonale vise de préférence une ligne déjà prévue qui touche ce bord (moins de 0,6 quad), sinon le point du bord lui-même.
  - Quand le coin d'une autre forme tombe presque sur le coin du carré (Cube 3 de la scène), on prend une autre ligne prévue à au moins 0,3 quad, si la diagonale y passe librement : pas de pincement ni de pointe.
  - Le carré de ces accroches est réservé comme les autres (une boucle venue d'ailleurs s'arrête en T sur ses côtés-lignes), et une boucle qui croise la diagonale d'une accroche s'y arrête en T.
  - Un côté du carré qui tombe à moins de 0,2 quad du bout d'une arête droite du bord passe par ce bout.
  - Deux faces voisines doivent voir le même carré (encoche partagée) : si une boucle s'arrête en T sur la diagonale d'une accroche, le Bake refait une passe avec ce côté du carré recalé sur le point d'arrêt (au plus quelques passes).
  - Scène de Romain : 692 quads, fermée, écart max 0,05 mm, et toujours fermée avec Cube 3 décalé de ±1 à 2 mm. Compromis : au-dessus du rond, la ligne du bas de Cube 3 prolongée passe à 0,87 mm du raccord (la géométrie l'impose).
  - Pas encore : le cylindre déplacé plus près du coin ou de l'encoche (cas `bossage-44/46/48/50` du probe : patch à 3 coins ou comptes impossibles).

### Base de règles sur les objets simples (robustesse, 10 nov.)

Demande de Romain : une fondation qui marche dans le maximum de cas simples avant d'ajouter les complexités une à une. Chaque règle est générale (aucune n'est réglée pour une scène) et se mesure avec `bake_robust` : une plaque de 20 × 10 × 20 mm et une forme (trou, trou borgne, cylindre posé, boîte, poche, avec ou sans raccord) déplacée au pas de 1 mm sur toute la face et au-delà du bord, deux formes, motifs de fillets, tailles, scènes de Romain. 5 939 cas.

| | propres | rangée fine | règle qui casse | pas encore géré |
|---|---|---|---|---|
| départ (1ffffe9) | 2 217 (37 %) | 3 % | 45 % | 14 % |
| aujourd'hui (5e75473) | 3 381 (57 %) | 2 % | 9 % | 33 % |

Le 10 nov. au soir, avec les cylindres arrondis : 4 familles de plus (bossage arrondi, avec raccord, fond de trou borgne arrondi, tailles de l'arrondi), 7 607 cas, 3 908 propres. Les familles arrondies passent exactement là où passent les mêmes formes vives (169, 121, 169 positions), aucune régression ailleurs. Puis le dessus traversé par une boîte : sa scène sans le raccord du cylindre passe, et avec Cube 3 après le cylindre (trois cas de plus dans « romain », 7 610 cas, 3 911 propres), aucune régression.

Une forme seule posée ou creusée sur la face passe à 100 % tant qu'elle reste à au moins 1 mm du bord. La boîte, la poche et le trou traversant passent aussi quand ils coupent le bord (sauf une boîte alignée sur deux parois à la fois, et un trou tangent au bord). Ce qui reste en « règle qui casse » : surtout deux formes qui se chevauchent.

**Les règles, dans l'ordre où elles jouent.**

1. **Faces dans le même plan** (une boîte alignée sur la paroi de la plaque) : la pièce d'avant garde sa face entière, la forme ajoutée commence à l'arête de cette face ; la boucle du bord continue tout droit sous la boîte. Un Carve aligné retire la face. Une face alignée n'est jamais « la face d'en face » d'une ligne : c'est sa voisine qui la porte. Contact face contre face (une boîte posée pile sur le dessus) : la face cachée disparaît.
2. **Coins de fillets** : trois arêtes arrondies du même rayon, coin en boule ; deux arrondies et la troisième vive, les deux bandes se coupent sur un onglet (plan bissecteur), rangées reliées une à une.
3. **Le carré d'un trou** (accroche en diagonale, à une case du coin des tangentes de chaque quart d'arc) :
   - un côté du carré à moins d'un demi-quad d'un bord droit se pose sur le bord (l'option A, en règle générale) ;
   - un coin de la face dans le quart du carré, à moins d'un quad et demi : la diagonale va dans ce coin ;
   - une boucle qui croise le chemin d'une diagonale posée au bord : la diagonale atterrit avec elle (moins de 30° d'écart) ;
   - une autre forme à moins de deux cases au-delà d'un côté : ce côté se place au milieu de l'écart (s'il fait au moins 0,3 quad), l'autre carré s'y cale ;
   - les deux bouts d'un trou traversant prennent le même carré.
4. **Trou qui coupe le bord (encoche en U)** : l'arc est coupé là où le cercle est carré aux axes de la face (ses points les plus larges, son fond) ; à un bout posé sur le bord, le côté du carré est perpendiculaire au bord. L'arc entre le bord et le point le plus large n'a pas de carré à lui : il prolonge le côté du suivant. Résultat : un U de boucles autour de l'encoche, quelle que soit sa profondeur.
5. **Bord arrondi d'un cylindre** (dessus d'un bossage, fond d'un trou borgne ; scène de Romain du 10 nov., 16 h) : une bande autour du cylindre (quart de tore, `Map::Tor`), comme la bande d'une arête de boîte. Elle suit la paroi : même couture cachée, ou mêmes deux moitiés quand la paroi se voit tout autour. Autour, ses deux rails prennent les segments de la paroi, si bien que le dessus en o-grid, la bande, la paroi et le raccord ont les mêmes colonnes. Les coupes en quarts du dessus passent sur l'autre rail de la bande (pareil pour l'anneau d'un raccord). Elle peut être coupée le long d'une rangée (le bossage posé sur son pied arrondi, le fond arrondi qui s'ouvre sur la face) ou en travers, d'un rail à l'autre (règle 6) ; entamée sur un seul rail, c'est « pas encore géré ». Il faut une paroi droite et un dessus plat (rayon plus petit que le rayon du cylindre et que sa demi-hauteur).
6. **Dessus d'un cylindre traversé par une boîte** (encoche dans le dessus ; même scène, Cube 3) : le dessus garde son o-grid. L'arc est coupé aux points où le cercle est carré aux axes (comme l'encoche en U), puis chaque morceau d'arc a son milieu au milieu de ses deux bouts, exactement là où le pied de la paroi prend son accroche : les colonnes de la paroi descendent droites du dessus au socle. Un bout sur la coupe se compte au rayon de la paroi, si bien qu'avec un bord arrondi la colonne traverse aussi la bande tout droit. Les coins du carré sont à 0,6 du milieu de leur quart ; le fond de l'encoche, à moins d'une demi-case du côté du carré, devient ce côté (sinon des lignes en partent à angle droit vers le côté). La bande arrondie s'arrête sur les faces de la boîte, sa rangée du milieu fait le tour d'une face à l'autre. Autour d'un cylindre ainsi coupé, les segments de la paroi ne sont plus fixes mais un minimum (les arcs entre les coupes ne partagent plus le tour en parts égales), et restent régulièrement espacés.
7. **Raccord d'un cylindre coupé net par une boîte ajoutée après** (même scène, Cube 3 placé après le cylindre dans l'historique) : la paroi a sa couture dans la partie cachée par la boîte, et l'anneau du raccord la suit (un seul anneau, même couture, au lieu de deux moitiés). Sur la face de la boîte, le bord qui passe sur le raccord prend le raccord (pas la plaque) ; le coin où la plaque devient raccord est sur le rail du raccord (les deux surfaces s'y touchent tangentes, le croisement direct serait imprécis). Sur la plaque, l'arc du rail est coupé comme le pied d'un cylindre vif (points carrés aux axes, coupes reportées sur l'autre rail) et ses bouts sur la coupe se comptent au rayon de la paroi : les colonnes du dessus, de la bande, de la paroi et du raccord sont les mêmes. Une ligne qui part d'un bord droit qui continue en douceur sur une courbe part d'équerre au bord droit, exactement.
8. **Cas limites refusés proprement** (« pas encore géré », jamais un maillage faux) : forme tangente à une face (paroi d'épaisseur nulle) ou à l'arrondi d'une arête ; forme qui entame l'arrondi d'une arête ; fond de trou borgne ou dessous de bossage coupé par le bord (il lui manque un quart d'arc) ; dessus coupé sur une diagonale de son o-grid, ou en biais ; bande arrondie entamée sur un seul rail.

**Outil.** `OUT=/tmp/x cargo run --release -p forme-core --example bake_robust` (environ 2 min) : une carte PNG par famille (une case = une position, couleur = classe), `results.tsv`, `résumé.txt` (messages les plus fréquents). Noms de familles en arguments (`trou`, `boite`, `fillets`…), `CAS="x 3 z 0"` pour un seul cas, `OBJ=fichier` pour son maillage. Toute nouvelle règle se valide sur le total et sur les scènes de Romain (aucune régression), en plus de `bake_probe`.

### Encore à faire (renvoie `Err` aujourd'hui)

- Deux formes qui se chevauchent (un trou qui mord dans une boîte, deux trous sécants) : patchs à 3 ou 5 coins.
- Trou borgne ou bossage coupé par le bord (fond en demi-disque) ; forme qui entame un arrondi ; formes tangentes.
- Raccord (fillet de jonction) près d'une arête ou d'une autre forme ; l'anneau d'un raccord à 0,5 mm d'une autre forme laisse des rangées fines.
- Fillets de jonction autres que la boîte posée sur une face (blend d'un Carve, de formes arrondies ou penchées), chanfreins, Intersect, Groove, Tongue.
- Sa scène du 10 nov., 16 h, complète : le raccord de 1 mm du cylindre s'applique aussi à Cube 3, qui est avant le cylindre dans l'historique (congés rentrants entre la boîte et la paroi, et entre la boîte et le dessus). Sans ce raccord, elle passe (bord arrondi : 734 quads ; bord vif : 678) ; avec Cube 3 après le cylindre aussi (766 quads). Cylindre arrondi sans paroi droite ou sans dessus plat ; chanfrein d'un cylindre.
- Sphères, profils, tores, coques, taper.
- Copies (répétitions, symétries), groupes fusionnés (historique en arbre).
- Coin où trois fillets de rayons différents se rejoignent ; coin arrondi coupé par une autre forme.
- Deux arêtes du dessus arrondies en coin, les autres vives (motif « 2 arêtes en coin ») : les côtés droits du bout de chaque bande font le tour de la pièce et reviennent buter sur l'autre bande juste à côté de l'onglet (comptes impossibles). Le motif voulu à ce coin reste à dessiner avec Romain. Le tour du dessus entier passe.
- Cube test et grenade : en attente de ces cas.

## Historique des approches (supprimées le 10 nov. 2026)

1. **Bake par surfaces** (`patch.rs`). Patchs paramétriques découpés et cousus. Précis, mais topologie refusée par Romain (« voxel converti »).
2. **Remaillage orienté champ** (`quad.rs`, style Instant Meshes). Fillets irréguliers, non-manifold. Abandonné.
3. **Bake par opération** (`build.rs`). Chaque opération refaisait une région autour de ses rails (anneaux de raccord, lofts). Cube test 1408 quads ; remplacé par le Bake par lignes, plus proche de ce que Romain veut (boucles qui suivent les lignes de construction).

Le code est dans l'historique git (commit avant le nettoyage du 10 nov.) si une idée doit être reprise.

## Outils de mise au point

- `OUT=/tmp/x cargo run --release -p forme-core --example bake_probe -- parlignes` : le Bake sur ses scènes de test, rendus CPU (`CASE=romain-aretes` pour une seule, `DEBUG_LAYOUT=1` pour les rayons tracés et les patchs refusés, `DEBUG_COUNTS=fichier` écrit le système des nombres de segments quand il ne s'accorde pas : une ligne par patch, `carte|côté…`, arête `e:nombre:fixé:cible:(a)-(b)`). Bandes de fillet en orange, remplissage en bleu.
- `… bake_probe -- lignes` : lignes de construction (`lines.rs`) sur la pièce bakée. `… bake_probe -- decime` : la passe optionnelle.
- `forme-shot -- /tmp/x 19` (cube test dans l'app : montre le message « pas encore géré »), `22` (fillets par arête, bake), `user` (scène de test de Romain).
