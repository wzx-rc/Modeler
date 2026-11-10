# Architecture

~17 000 lignes de Rust + un shader WGSL, en trois crates (workspace Cargo).

```
crates/
  forme-core/     noyau sans dépendance plateforme
  forme-render/   rendu wgpu du viewport
  forme-app/      application egui (iPad + PC) et harnais de captures
ios/              Info.plist, icônes, build_ipa.sh
.github/workflows/build.yml   .ipa (macOS) + tests (Linux)
```

## forme-core

| Fichier | Contenu |
| --- | --- |
| `scene.rs` | `Scene` (arbre de `Node` indexé par `NodeId`, racine `scene.root`). Nœud = forme (`Shape`) ou groupe. Il porte une `Transform` (position, rotation, échelle), un `Combine` (mode Add/Carve/Intersect/Groove/Tongue, `blend`, `kind` lisse ou chanfrein, `depth`/`width` pour Groove/Tongue), une `Repeat` (radiale ou linéaire), un `Mirror` XYZ, une matière et la visibilité. `Shape` contient : `kind` (`PrimKind`, 9 types), `size`, `round`, `chamfer`, `taper`, `shell`, `sides`, `profile`, **`edges`** (fillets par arête). Constructeurs : `NodeSpec::shape(..).at(..).rot(..).mode(..).blend(..).mat(..)` et `NodeSpec::group(name, fuse)`. Groupe `fuse = false` : les enfants se combinent directement avec l'extérieur (transparent). |
| `program.rs` | Compilation de la scène en **bytecode SDF** (`Program { code, aux }`, mots `[f32;4]`). Ops : EMPTY, RESET, AFFINE, RADIAL, LINEAR, MIRROR, PRIM, COMBINE, BOUND (sphère englobante pour sauter une forme lointaine), END. Les répétitions évaluent la copie la plus proche **et la voisine** (`second`), pour éviter les coutures. Interpréteur CPU f32 : `eval`, `dist`, `normal`, `raycast`. `compile(scene)` et `compile_single(scene, id)`. `box_edges` / `cyl_edges` évaluent les fillets par arête. |
| `edges.rs` | Arêtes sélectionnables pour les fillets. `EdgeIndex::new(scene)` construit un champ f64 par forme visible. `pick(scene, p, tol)` renvoie un `EdgePick` : soit une **jonction** entre deux formes (courbe coin tracée par prédicteur-correcteur, puis posée au milieu du fillet), soit **une arête propre** d'une forme (boîte 12, cylindre 2, sur toutes ses copies). `refresh` recalcule après une édition. `junction_owner` trouve le nœud dont le blend fait la jonction. |
| `history.rs` | Undo/redo par instantanés de scène. |
| `mesh.rs`, `export.rs` | Dual contouring à arêtes vives (ancien export), OBJ et GLB, matières en couleurs de vertex. |
| `demo.rs` | Scènes de démo (grenade sci-fi), matières. |
| `bake/` | Tout le Bake (voir `BAKE.md`). |

### bake/

| Fichier | Rôle |
| --- | --- |
| `field.rs` | Réinterprète le bytecode en **f64**. `Field::eval` renvoie `Sample { d, mat, owner, margin, blend }`, avec `dist`, `grad` et `project` (Newton). `prims: Vec<PrimDef>` et `instances(prim)` donnent les copies avec leurs `to_local` / `to_world`. **`steps()`** donne l'historique à plat (`Step { prim, mode, k, chamfer, end }`) et **`prefix(steps, n)`** le modèle après n étapes. On y trouve aussi `prim_raw`, `prim_project`, `box_edges`, `cyl_edges` et `box_edge_signs`. |
| `gen.rs` | Maillages **natifs** des primitives (graines des lignes de construction, `prim_bounds`, `ear_clip`) : `rounded_box` (dont `rounded_box_edges` pour les fillets par arête), `quad_sphere`, `extrusion`, `revolve` (disques tout en quads `quad_disc` ; `GenSettings.segs` force le nombre de colonnes). `PolyMesh { pos, faces }`. |
| `layout.rs` | **Le Bake** (seul algorithme) : par lignes de construction. `build(scene, settings, progress) -> Result<Baked, String>`, `handles(scene)`. Cartes par face, bords exacts, boucles, patchs à 4 coins, TFI (voir `docs/BAKE.md`). |
| `lines.rs` | Lignes de construction lues dans l'historique (arêtes, rails, jonctions), pour les calques de diagnostic du panneau Bake. |
| `decimate.rs` | Passe optionnelle (désactivée par défaut) : retire les edge loops plates. |
| `mod.rs` | `bake()` = `plan` (taille des quads, ou `Err(raison)`) puis `layout::build`, sans repli. Contient aussi `Baked`, `BFace` (rôle `P_FILLET` / `P_FILL`), `BakeStats`, `Quality`/`BakeSettings::preset` (densité : `divisions` = 5 / 8 / 12 quads sur la forme de base), `finish` (normales, stats), `to_obj`, `to_mesh`, `triangulate_face`. |

## forme-render

- `sdf.wgsl` : raymarcher du bytecode (ombres, AO, grille, sélection, fantôme des formes de coupe, `hide_scene` pendant l'aperçu du bake). **Même sémantique que l'interpréteur CPU.**
- `lib.rs` : `Renderer` (`upload_program`, `update_view(view, settings)`, `draw(pass)`), `View`, `Settings`, `SelMode`.
- `mesh.rs` : `MeshRenderer`, aperçu du maillage baké (éclairage, filaire des vraies arêtes de polygones).
- `headless.rs` : rendu hors écran (tests, captures).

## forme-app

| Fichier | Contenu |
| --- | --- |
| `main.rs` | Boucle winit, surface wgpu, egui-winit. Le **bouton pouce Carve** et le **bouton CTRL** sont gérés hors egui (un doigt les tient pendant que le Pencil agit). Redessine seulement quand c'est utile. Sauvegarde « Session » au passage en arrière-plan. Sur iOS : budgets de pixels et `UI_ZOOM = 0.84`. |
| `state.rs` | `AppState` : scène, sélection, outils (`Tool::Select`, `Place(PrimKind)`, `Profile`, `Kit`), placement façon 3ds Max (`Placement`, `PlacePhase` Base / AwaitHeight / Height, `offset` coin à coin), poignées (`Handle`, dont `Fillet`), sélection d'arêtes (`edge_hover`, `edge_sel: Vec<EdgePick>`, `ctrl_held` / `key_ctrl`, `set_fillet`), bake en tâche de fond (`BakeState`), export, historique, toasts. |
| `viewport.rs` | Gestes (doigt, Pencil, souris), dessin des poignées, des arêtes et des puces. |
| `ui.rs` | Toute l'interface : barre du haut, colonne d'outils, panneau droit (`RIGHT_W` = 276), propriétés, carte « Arête sélectionnée », arbre de formes, dialogues Bake / Export / Documents, boutons Carve et CTRL. |
| `gfx.rs` | `FrameRenderer` : image **rapide** (en mouvement, résolution adaptée au temps de frame) et image **fine** (raffinée par bandes avec un scissor, affichée une fois complète), puis blit et egui par-dessus. |
| `camera.rs`, `theme.rs`, `icons.rs`, `files.rs`, `kit.rs` | Caméra orbitale et ortho, couleurs et polices, icônes vectorielles, documents (`Documents/Forme` sur PC, Fichiers sur iPad), kit de pièces. |
| `bin/shot.rs` | **Harnais de captures** : l'app réelle sans fenêtre, entrées simulées (tap, glisser, survol), PNG. Scénarios nommés (`02`…`29`, `max`, `user`, `live`, `ui-ipad`…). |

## Flux d'une frame

1. Entrées : egui reçoit les touches ; `viewport::handle_input` lance gestes, outils et poignées.
2. `AppState` modifie la scène (`changed()` incrémente `scene_version`) ou la vue (`view_changed()`).
3. `gfx::FrameRenderer::render` recompile et téléverse le bytecode si besoin, puis rend l'image rapide ou une bande de l'image fine, la blit, et dessine egui.
4. Bake : `start_bake()` lance un thread `bake::bake`. Le résultat (`Arc<Baked>`) est affiché par `MeshRenderer` à la place du raymarching.
