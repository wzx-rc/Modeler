# CLAUDE.md — Forme NURBS (branch `claude/nurbs-zero`)

Instructions for Claude (Claude Code on PC, or a claude.ai Project) working on this repository.

**This branch is Forme NURBS restarted from zero** (Romain, 17 nov. 2026: "repartir à 1000 % de zéro… il faut absolument tout enlever du SDF"). It holds no SDF code at all: no raymarcher, no SDF scene, no Bake, no SDF edge picking. Forme SDF and the first Forme NURBS (built on top of the SDF app) live on the branch `claude/nurbs`; `docs/` still describes Forme SDF and its Bake. Never bring SDF code back here, not even as a preview.

## The project in three lines

Forme NURBS is a hard-surface modelling app (PC first, iPad later): an object is a list of features (Cube, Cylinder…) combined in order (Add = union, Carve = cut), built as exact B-rep solids by **OpenCascade** and shown as its mesh with its B-rep edges. The UI looks like Forme's « Établi » interface (floating islands, tool cards, one inspector, history strip), in English, dark, with the white accent.

## Working with Romain

- Always reply in **French**: short, straight to the point, with screenshots when there is something to see. He often reads on his phone.
- The UI text is in **English**. Code comments are in English. Commit messages are in French (title + bullet points).
- Move forward **step by step** on simple cases. Show a drawing or a capture before a large change of direction.
- Be honest about limits: say what has only been checked in captures, and what has not been tried at the mouse or on a real device.

## Plan (announced to Romain on 17 nov. 2026)

1. **Base** (done): viewport, grid and world origin, Cube and Cylinder in Add / Carve, selection and move; OpenCascade recomputes the booleans live during every drag.
2. **Edges** (done): Edges mode (2) picks the real B-rep edges; fillets and chamfers are history steps that find their edges again by face tags.
3. **Quick creation, materials, Sketch** (done, captures only): the tool's key again (or Shift + key) places a 20 mm shape at the origin, on top of the selection, or drilled through it in Carve; 6 material balls in a folded section; Sketch (L) draws an outline of corners and curve points on the flat face under its first click (else the ground), closed on its first point or with Enter, then pulled to a height. **Workplanes were tried and dropped by Romain (17 nov., 19 h): do not bring them back unless he asks.**
4. **Quads** (asked 17 nov., 18 h 37: "un premier convertisseur polygonal sur un objet simple avec fillet", then 19 h 09: "Continue de pousser"; captures only): the Quads view (button in the view island) shows the part as the kernel's quad mesh, fillet and chamfer bands in orange, and exports it as .obj. Each B-rep face is cut into 4-cornered regions by straight cuts; the test part (hole, boss, slot, a fillet) comes out all in quads. Romain's rules for the mesh (from Forme SDF's Bake): fillets first as bands of quad rows, then the flat faces filled with straight rays and no poles; around holes and bosses a ring of rays with diagonals to the corners. Still to do: a disc (the boss top) is one 4-sided patch, not yet a centre square with a ring.

## Commands

```bash
export FORME_OCCT_ROOT=/path/occt-7.8                        # an installed OCCT 7.8 (lib/ include/); on PC use --features builtin instead
cargo test --release -p forme-occt --no-default-features      # the C layer: cube − hole, fillet, face tags, topology
cargo test --release -p nurbs-kernel --features occ           # the document → B-rep build, caches, skip, encode / decode
cargo build --release -p nurbs-kernel --features occ          # the kernel process forme-nurbs-kernel
cargo build --release -p nurbs-kernel --features builtin      # same, building OpenCascade from source the first time (6 to 20 min, cmake + C++ compiler)
cargo build --release -p nurbs-app                            # the app forme-nurbs and the capture harness forme-nurbs-shot
cargo run --release -p nurbs-app --bin forme-nurbs            # the app (finds target/release/forme-nurbs-kernel next to it, or FORME_NURBS_KERNEL=path)
./target/release/forme-nurbs-shot /tmp/x                      # every capture scenario on the test part; prefix filter: empty, part, move, create, quick, sketch, edges, quads
```

`forme-nurbs-shot` runs the real app headless (egui + wgpu, real kernel process, real mouse events). It accepts a software GPU (lavapipe / llvmpipe) and writes full-quality PNGs at 1600×900 points, scale 2. Use it to check every visual change before showing it.

The window title shows the version, branch, commit and commit date (`crates/nurbs-app/build.rs`, `nurbs_app::version_title()`): check it before saying which version Romain is running.

## Code map

- `crates/forme-occt`: our thin C layer over OpenCascade (`cpp/forme_occt.cpp` + safe Rust wrapper). Primitives, transforms, booleans, fillets / chamfers by edge index, meshing. Every OpenCascade exception is caught and returned as `Err`; only `opencascade::handle<T>` is used (the `Handle_T` aliases break under MSVC). **Face tags**: each face carries an `i64` tag that booleans, transforms, fillets and chamfers carry to the faces they make (`carry_tags`, history `Modified`); new fillet / chamfer faces get the `fresh` tag. `Shape::topo(defl, angle)` returns in one buffer the faces (tag, surface kind, plane point / normal, axis, radius), the mesh with a face index per triangle, and the edges (index, the two faces, kind, seam, polyline). OpenCascade is built by `occt-sys` (feature `builtin`, cached in `target/OCCT`) or taken from `FORME_OCCT_ROOT`.
  Quads: `quad_topo()` (each face's wires in order, each edge's ends, kind and samples), `edge_on_face(face, edge, reversed, t)` (points and the face's uv along an edge), `face_eval(face, uv)` (points and outward normals).
- `crates/nurbs-kernel`: the document and the kernel.
  - `doc.rs`: `Doc { features }`, `Feature { id, name, op, prim, origin, rot, visible, material, round }`. A feature's origin is the centre of its base; it rises along its local +Y. Saved as JSON `.fnurbs`. A feature with `round: Some(Round { size, chamfer, edges })` is a fillet / chamfer step on what is built before it (`is_solid()` is false; its `op`, `prim`, `rot` are unused). Each edge is an `EdgeRef { faces: [tag, tag], at }`: the packed tags of the two faces it joins, and its middle point to choose between edges joining the same two faces (`build::resolve`). The fillet's new faces carry the tag `(fillet id, 0)`, so clicking them selects the fillet. A sketch is `Prim::Profile { x, y, z }` (its outline's box, its height) with `profile: Some(Profile { points, smooth })`, the points in the unit square of local X / Z (resizing stretches it); a curve point is a quadratic curve between the middles of its two sides (`doc::pieces`, same as `focc_prism`).
  - `model.rs`: what the kernel sends back: mesh, `tri_face`, faces with their `FaceTag { feature, sub }` (box sub: 0 +X, 1 −X, 2 top, 3 bottom, 4 +Z, 5 −Z; cylinder: 0 side, 1 top, 2 bottom), edges, errors; binary encode / decode.
  - `build.rs` (feature `occ`): features applied in order (a Carve with nothing before it is ignored), tool solids cached by feature hash, stages by prefix hash, a sketch's faces tagged 1 top, 2 bottom, 3 + its outline piece id (`sub_of`), `STEP what:id:hash` written on stderr before each risky step.
  - `quads.rs` (feature `occ`): the quad converter. `convert(shape, target)`: each face's loops in a (u, v) map scaled to millimetres (`FMap`, the outline counter-clockwise; for a closed edge the pcurve that keeps the loop going is taken, `Span.pc`), cut into regions with four corners (outline turns > 25°) by straight cuts that stop at the first line they meet (a T), on a node when one is close. In order: several holes kept apart by a cut across the face (`separate`); each hole joined by a left and a right ray; then per region a ray along a side of an inner corner, a ray from an inward arc of more than 120° (where it runs along u or v), a ray straight in where a straight side meets a rounded one, the best chord while more than 4 corners (`score`: corner counts, angles, no sliver, no cut brushing past the outline, `clear`), forced corners while fewer (where a straight side meets a rounded one, else the middle of the longest run). The lines (B-rep edges and cuts) are split into pieces at their nodes; segment counts by length (arcs at least one per 22.5°), raised until opposite sides match; points shared by node / piece, a Coons grid in uv evaluated on the surface, quads turned outwards. `QuadMesh::is_closed`, `bad_quads`, `obj`; `FORME_QUADS_DEBUG=1` prints the cuts. Asked with `Request.quads` (fine builds only); sent back as `Model.quads` (`Quads { positions, quads, quad_face, skipped }`, appended at the end of the encoding).
  - `remesh.rs` (branch `claude/quad-remesh`, the « Export converter », separate from `quads.rs`): field-aligned quad remesh of the whole part after Instant Meshes (written from the paper; QuadRemesher only studied from its scripts and settings). Welded kernel triangles refined to a third of a quad and put back on the exact surface / B-rep edge; 4-RoSy orientation field fixed along B-rep edges and on round faces (axis / circle direction); position field carried out from the corners then smoothed; samples on one lattice point merged, graph faces traced, each n-gon cut into n quads. `Settings { target_quads }`, `remesh(  - `client.rs`: the app side.Model)`, `obj`, `is_closed`; `FORME_REMESH_DEBUG=1`. Tests write `forme-remesh-*.obj` (`FORME_REMESH_OUT`, `FORME_REMESH_TARGET`).
  - `client.rs`: the app side. The kernel runs in its own process `forme-nurbs-kernel` (u32-length frames on stdin / stdout, JSON request, binary model): a hard crash inside OpenCascade cannot take the app down; the client restarts it with the failing step skipped and the model lists what was left out. The latest request wins; coarse meshing while a gesture is live, fine after.
- `crates/nurbs-app`: the app.
  - `render.rs`: wgpu, MSAA 4×: background (gradient, ground grid 10 / 50 mm, origin axes), the part's mesh with per-face flags (selected / hovered) in a storage buffer, B-rep edges as anti-aliased screen-width lines, overlay outlines (cutters, shapes being moved or created) with a dimmed hidden part.
  - `pick.rs`: ray against the kernel's triangles, face by face → the face's tag → the feature. No SDF, no approximation.
  - `state.rs`: `AppState` (document, undo / redo with live edits, selection, tools, 3ds Max-like creation on the ground or a flat face, move gizmo and size handles, kernel round trips). Features go here first. `SelMode::{Object, Edge}` (Q or 1 / 2, the EDGES card): in Edge mode `pick_edge` finds the nearest visible model edge on screen, `edge_sel` holds the picked `EdgeRef`s, `set_round` makes the fillet at the first size (or sets the selected one's), Shift + click on an edge adds it to the selected fillet. `Sketch` (with `Tool::Create(PrimKind::Profile)`): `sketch_press` / `sketch_drag` / `sketch_close`, then the same height phase as the other shapes (`Creation::profile`, mirrored when it grows down).
  - `viewport.rs`: gestures (Maya navigation: Alt + left orbit, Alt + middle pan, Alt + right zoom, wheel zoom), gizmo, creation and marquee drawing, kernel status line.
  - Quads view: `AppState.quads` (the request serial is `version * 4 + fine * 2 + quads`), `quad_mesh()`, `export_obj()` (Documents/Forme NURBS/<name>.obj, one group per face); `draw.rs` uploads the quads (smooth normals inside a face), their wire, the skipped faces' triangles, and colours fillet faces `BAND` orange.
  - `ui.rs` + `widgets.rs` + `theme.rs` + `icons.rs`: the Établi-style islands, inspector, history strip, files dialog; `draw.rs`: what is uploaded to the renderer each frame; `materials.rs`: the 6 materials (colour, metal, roughness) read per face by the shader; `files.rs`, `settings.rs`: `~/Documents/Forme NURBS`.
  - `bin/shot.rs`: the capture harness (`test_part()`: Block, Hole, Boss, Slot).

## Rules that are easy to break

- **No SDF, ever, on this branch**: the part on screen is always the kernel's mesh. While the kernel computes, the previous mesh stays, with the moved or created shape drawn as an outline.
- Picking and selection go through the kernel's faces and their tags (`Model::faces[f].tag`), never through a field or a guess from the shape list.
- A tag is `(feature << 16) | sub`; any new primitive must tag all its faces in `nurbs_kernel::build::sub_of`. Edges are never named by their index in OpenCascade's edge map (it changes with every build): always by the tags of their two faces.
- Both sides of the kernel protocol (`proto.rs`, `model.rs` encode / decode) change together; the kernel process is rebuilt with the app.
- Never commit `target/` or `shots/replay/frames/`. Never run `cargo fmt` on the whole tree.
