# CLAUDE.md — Forme

Instructions for Claude (Claude Code on PC, or a claude.ai Project) working on this repository.
Read `docs/00-PROJET.md` first: vision, decisions already taken with Romain, state, what's next.

## The project in three lines

Forme is a hard-surface modelling app for iPad Pro, in the spirit of Substance 3D Modeler without VR: an object is built by combining parametric SDF shapes (Add, Carve, Intersect, Groove, Tongue). It is close to CAD, not sculpting. Everything is written in Rust (egui + wgpu), and the same code runs on iPad (Metal) and on PC (DX12 / Vulkan). The **Bake** turns the parametric history into a clean, all-quad export mesh, the way it would be modelled by hand.

## Working with Romain

- Always reply in **French**: short, straight to the point, with screenshots when there is something to see. He often reads on his phone.
- All UI text is in French. Code comments are in English. Commit messages are in French (title + bullet points).
- Move forward **step by step** on simple cases: the 20 mm test cube before complex scenes like the grenade. Show a drawing or a capture before a large change of direction.
- Topology requirements for the Bake are strict: quads, loops that follow the shapes, fillets as rows like a bevel modifier, no "voxel converted" look. See `docs/BAKE.md`.
- Be honest about limits: say what has not been tested on a real device.

## Commands

```bash
cargo test --release -p forme-core                      # core tests (25 tests)
cargo run --release -p forme-app --bin forme            # the app on PC
cargo run --release -p forme-app --bin forme-shot -- shots          # every capture scenario
cargo run --release -p forme-app --bin forme-shot -- /tmp/x max     # one scenario (prefix): max, live, 22, 19, ui-ipad…
cargo run --release -p forme-app --bin forme-shot -- /tmp/x pc-atelier pc-studio   # the two PC interfaces at 1600×900
IPAD_ZOOM=0.84 cargo run --release -p forme-app --bin forme-shot -- /tmp/x ui-ipad   # layout as on the iPad
OUT=/tmp/x cargo run --release -p forme-core --example bake_probe -- parlignes  # the Bake on its test scenes, CPU renders (CASE=romain-aretes|rc|raccord|boss|bossage|bossage-c3z+1|rc-sans-raccord|ra-coin3|romain|coin|trou|… for one, DEBUG_LAYOUT=1 for the rays, DEBUG_FACE="prim face file" dumps one face's lines, DEBUG_COUNTS=file dumps the segment-count system when it does not settle, QUALITE=legere|dense, BEVEL=0 for the layout without the sharp-edge bevel, OBJ=1 writes each mesh)
OUT=/tmp/x cargo run --release -p forme-core --example bake_probe -- lignes     # construction lines (bake/lines.rs) over the baked part
DEG=5 OUT=/tmp/x cargo run --release -p forme-core --example bake_probe -- decime  # optional pass: dissolve the closed edge loops whose cross path bends less than DEG (same code as the toggle)
OUT=/tmp/x cargo run --release -p forme-core --example bake_robust              # robustness: ~6000 simple cases (plate + one shape moved on a 1 mm grid, fillet patterns, sizes, Romain's scenes), maps + results.tsv + résumé.txt; family names as args (trou, boite, fillets…), CAS="x 3 z 0" OBJ=file for one case, BEVEL=0 grades the layout alone (the reference for layout rules)
```

## Two apps: Forme SDF and Forme NURBS (Romain, 17 nov. 2026)

The same `forme-app` code builds two apps that Romain works on separately:
- `forme` = **Forme SDF** (French UI, the SDF raymarched part);
- `forme-nurbs` = **Forme NURBS** (English UI, dark with the white accent « Blanc »): same tools and UI, but the part is built as B-rep solids by a CAD kernel and shown as its mesh (the SDF preview stays while the kernel computes). `forme_app::nurbs()` is set from the exe name or `FORME_ENGINE=nurbs`; settings in `settings-nurbs.json`; UI strings go through `crate::i18n::{tr, trd}` (French source text, English dictionary), `FORME_LANG=en|fr` overrides.
- A tool built for one app is ported to the other by removing its `nurbs()` gate: put features in `state.rs` like any other tool, never in a copy of the UI.

The NURBS kernel is **OpenCascade**, chosen by Romain over truck (whose booleans fail on sphere / torus faces and whose fillets do not work); what matters to him is that it can go to the iPad. `crates/forme-occt` is our own thin C layer over it (`cpp/forme_occt.cpp` + safe Rust wrapper): primitives, booleans with their junction edges, fillets / chamfers by edge index, meshing; every OpenCascade exception is caught and returned as `Err`; only `opencascade::handle<T>` is used (the `Handle_T` aliases are a derived class under MSVC, which is why the `opencascade` crate does not build on Windows). OpenCascade itself is built from source by `occt-sys` (feature `builtin`, cached in `target/OCCT`) or taken from `FORME_OCCT_ROOT`. On PC it runs in a separate process, `forme-occ` (crate `nurbs-occ`), next to the app executable (or `FORME_OCC=path`): a hard crash inside OpenCascade cannot take the app down, `nurbs_core::worker::build_occ` reruns it with the failing step skipped (`--skip fillet:<id>`…), and what was left out is listed. Without `forme-occ`, the app falls back to the pure Rust kernel (`nurbs_core::build`, truck). On iPad (no child processes) forme-occt will be called in-process.

Mapping: Add = union, Carve = cut, the node's Blend (Raccord) = fillet of the boolean's new edges (Chamfer kind = chamfer), local junction fillets = fillet of the new edges along the zone, Round / Chamfer / per-edge fillets of a box or cylinder = fillets of the primitive's own edges, repetitions and mirrors = copies.

NURBS-only tools (gated by `nurbs()`, ported by removing the gate): **quick creation** (`AppState::arm_tool` / `quick_add`: the armed tool again, or Shift + C/S/A/T, places a 20 mm primitive at the world origin, stacks it on the selected shape's top along its axis, or drills through it in Carve), the **world origin** on the grid (`Settings.origin`, red X / blue Z axes and a ring in `sdf.wgsl`), and Modo-like **workplanes** (`state::Workplane`, `workplane_candidates()`: Ground, the vertical plane facing the camera, the top of the selection; drawn in `sdf.wgsl` with depth from `Globals.planes`, yellow, hovered = brighter; `begin_placement` starts on the plane under the pointer unless a face of the part is nearer; P or the Workplane button toggles them). The **Sketch** tool (L, Romain 17 nov.: 2D creation; `Tool::Sketch`, `state::Sketch`): click = corner, drag = curve point, click the first point or Enter to close (Backspace removes the last point, Esc drops it); the outline is drawn on the workplane under the pointer, else the face, else the ground, then pulled like a primitive's base (up = Add, down = Carve) into a `PrimKind::Profile` (Extrude) shape, which forme-occ builds as an exact prism (`Shape::prism`: smooth points are quadratic Bézier curves).

```bash
cargo build --release -p nurbs-occ                       # the kernel process; builds OpenCascade 7.8 from source the first time (6 to 20 min, needs cmake + a C++ compiler)
FORME_OCCT_ROOT=/path/occt cargo build --release -p nurbs-occ --no-default-features   # same, against an installed OCCT 7.8 (lib/ include/)
FORME_OCCT_ROOT=/path/occt cargo test --release -p forme-occt --no-default-features   # the C layer: cube − hole, fillet of the rims, volume
cargo run --release -p forme-app --bin forme-nurbs       # Forme NURBS (uses target/release/forme-occ when it is there)
OUT=/tmp/x cargo run --release -p nurbs-core --example occ_test     # the kernel on small parts (cube + hole + fillets…), OBJ per case
cargo run --release -p forme-app --bin forme-shot -- /tmp/x nurbs nurbs-stack nurbs-planes nurbs-sketch   # NURBS captures (fillet test part, bearing, primitives; quick creation and stacking; workplanes; sketch + extrude)
```

The window title shows the version, branch, commit and commit date (`crates/forme-app/build.rs`, `forme_app::version_title()`), also at the bottom of Réglages: check it before saying which version Romain is running.

`forme-shot` runs the real app headless (egui + wgpu). It accepts a software GPU (lavapipe / llvmpipe) and writes full-quality PNGs. Use it to check every visual change before showing it.

The iPad `.ipa` is built by GitHub Actions on every push to `main` (repository `wzx-rc/Modeler`). It is published as the `ipa-latest` release. Details are in `docs/DEV-PC.md`.

## Code map (details in `docs/ARCHITECTURE.md`)

- `crates/forme-core`: scene (shape tree), SDF bytecode compiler + CPU interpreter, history, `.forme` JSON projects, OBJ/GLB export, dual contouring, `edges.rs` (edge picking for fillets), `bake/` (the Bake by lines `layout.rs`, construction lines `lines.rs`, optional pass `decimate.rs`, f64 interpreter `field.rs`, primitive tessellation `gen.rs`).
- `crates/forme-render`: WGSL raymarcher (`sdf.wgsl`), preview of the baked mesh (`mesh.rs`), headless capture.
- `crates/forme-app`: two desktop interfaces chosen in Settings (redesign of 10 nov. 2026): « Établi » (`ui.rs`, enum `UiMode::Atelier`: viewport first, floating islands, tool column, one inspector, context bar over the selection, history strip; also the iPad layout) and « Studio » (`studio.rs`: docked CAD station like Plasticity, menus, command palette on Espace, sizes panel typed while a creation tool is armed via `AppState::place_sized`, fixed side panel scene / properties / history, status bar) (« Radial » was removed on 10 nov.; a saved "Radial" opens as Établi) and « Focus » (`ui.rs::show_focus`: dock at the bottom, radial menu, scene drawer), plus Romain's two moods « Plaque » (`plaque.rs`: thin tool bar left, floating inspector in mm, docked scene tree, status bar; orange) and « Radial » (`radial.rs`: tool pill with ADD / CARVE, tool cards on the right, contextual inspector, JetBrains Mono labels; green); the ambiance (accent colour, `theme::set_ambiance`) is Auto (Établi and Radial green, Studio and Plaque orange, Focus blue) or one for all, in Réglages; colours in `theme.rs`; user preferences (`settings.rs`, `settings.json` next to the projects, `FORME_UI=atelier|studio|focus|plaque|radial` overrides); gestures (`viewport.rs`), state and tools (`state.rs`), frame composition and progressive rendering (`gfx.rs`), entry point (`main.rs`), capture harness (`bin/shot.rs`).

## Rules that are easy to break

- **The shader (`sdf.wgsl`), the f32 CPU interpreter (`program.rs::eval_prim`) and the f64 bake interpreter (`bake/field.rs::prim_raw`) must stay identical.** Any change to a primitive (for example per-edge fillets) goes in all three.
- Local junction fillets (`Node.blend_zones`, `BlendZone::k_at`): the blend of a combine is `max(own blend, zone radius fading with the distance to the zone's corner polyline)`, evaluated at the world point in `sdf.wgsl::zone_k`, `program.rs::zone_k` and `bake/field.rs::zone_k` (same formula, same aux layout). The Bake refuses them for now. A zone is the straight run of the junction's raw corner curve around the pick (`edges::corner_piece`).
- Box edge indices (12): `axis*4 + k`, with k over the signs of the other two axes `(+,+) (-,+) (-,-) (+,-)`, other axes = `(axis+1)%3, (axis+2)%3`. Cylinder: 0 = top rim, 1 = bottom rim.
- `Shape.edges` empty = uniform fillet (`round` / `chamfer`). Negative value = chamfer. `set_edge` collapses back to uniform when every edge is equal.
- Bake build order (validated by Romain): **fillets first**, as quad strips shown in orange in debug renders, **then the fill paths** (flat faces, walls) between them, reusing the strips' edge vertices exactly. See `docs/BAKE.md`.
- The pair bevel on sharp edges (`bake/bevel.rs`, Romain's Maya reference) is **part of `bake()`**, always on: width `BakeSettings.bevel` (0.25 mm, 0 = none). A corner it cannot bevel returns `Err`.
- The flat-loop pass (`bake/decimate.rs`) is an **option, off by default** (validated by Romain): never call it from `bake()`. It runs only when the toggle in the Bake window is on, on a copy of the result.
- **One Bake algorithm: by construction lines (`bake/layout.rs`)** (Romain, 10 nov. 2026). The old bakes (by operation `build.rs`, by surfaces `patch.rs`, field-aligned `quad.rs`) are deleted: never bring back a fallback mesh. A history it does not handle returns `Err(reason)`; `bake()` returns it as is and the Bake panel shows it in red, with no mesh. Never return a broken mesh in silence.
- Renderer: an image is "moving" when it changes from frame to frame (not just because a tool is active). The full-quality image is refined in bands, so it never blocks the UI.
- Both interfaces drive the same `AppState`: a feature goes in `state.rs`, then gets a control in `ui.rs` (Atelier) and in `studio.rs` (Studio). Each interface sets `st.view_rect` every frame (the free 3D area), which centres the camera.
- The app creates only Cube, Cylinder, Sphere, Capsule and Torus (`state::PRIMS`) with the Add and Carve modes (`state::MODES`). Forme NURBS adds the Sketch tool (extruded profiles), asked by Romain for the NURBS app on 17 nov.; Forme SDF keeps the list as is. The Profile tool, the kit of parts, repetitions, Cone / Prism / Tube, Intersect / Groove / Tongue and the old "advanced" setting were removed from the app (Romain, 10 nov. 2026): do not bring them back as hidden options. forme-core still knows them, so older `.forme` files load and render.
- Never commit `target/` or `shots/replay/frames/`.
