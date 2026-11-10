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
cargo test --release -p forme-core                      # core tests (20 tests)
cargo run --release -p forme-app --bin forme            # the app on PC
cargo run --release -p forme-app --bin forme-shot -- shots          # every capture scenario
cargo run --release -p forme-app --bin forme-shot -- /tmp/x max     # one scenario (prefix): max, user, live, 22, 19, ui-ipad…
cargo run --release -p forme-app --bin forme-shot -- /tmp/x pc-atelier pc-studio   # the two PC interfaces at 1600×900
IPAD_ZOOM=0.84 cargo run --release -p forme-app --bin forme-shot -- /tmp/x ui-ipad   # layout as on the iPad
cargo run --release -p forme-core --example bake_probe -- build      # bake of the test cube → /tmp/build_a.png, _b.png
cargo run --release -p forme-core --example bake_probe -- variants   # 7 bake variants → /tmp/var_*.png
cargo run --release -p forme-core --example bake_probe -- screws     # box + 4 kit screws
cargo run --release -p forme-core --example bake_probe -- target     # hand-made target topology (reference)
OUT=/tmp/x cargo run --release -p forme-core --example bake_probe -- lignes   # test part: junction lines + fillet strip alone
OUT=/tmp/x cargo run --release -p forme-core --example bake_probe -- anneau   # test part: front ring of the bearing (bore fillet + outer round)
OUT=/tmp/x cargo run --release -p forme-core --example bake_probe -- support  # test part, junction by junction (par opération or why not)
OUT=/tmp/x cargo run --release -p forme-core --example bake_probe -- raccord  # test part: bearing front face joining the two rings (topologies A and B)
OUT=/tmp/x cargo run --release -p forme-core --example bake_probe -- liaison  # test part: front face (A) welded to the bearing / base fillet
OUT=/tmp/x cargo run --release -p forme-core --example bake_probe -- parlignes  # bake from the construction lines (bake/layout.rs): Romain's scene, without fillet, box with one rounded edge, cylinder carved in a corner (CASE=coin for one)
OUT=/tmp/x cargo run --release -p forme-core --example bake_probe -- piece    # test part: the whole part, fillets first (orange) then fill paths (cream), closed
DEG=5 OUT=/tmp/x cargo run --release -p forme-core --example bake_probe -- decime  # dissolve the closed edge loops whose cross path bends less than DEG out of the surface (same code as the optional pass)
```

The window title shows the version, branch, commit and commit date (`crates/forme-app/build.rs`, `forme_app::version_title()`), also at the bottom of Réglages: check it before saying which version Romain is running.

`forme-shot` runs the real app headless (egui + wgpu). It accepts a software GPU (lavapipe / llvmpipe) and writes full-quality PNGs. Use it to check every visual change before showing it.

The iPad `.ipa` is built by GitHub Actions on every push to `main` (repository `wzx-rc/Modeler`). It is published as the `ipa-latest` release. Details are in `docs/DEV-PC.md`.

## Code map (details in `docs/ARCHITECTURE.md`)

- `crates/forme-core`: scene (shape tree), SDF bytecode compiler + CPU interpreter, history, `.forme` JSON projects, OBJ/GLB export, dual contouring, `edges.rs` (edge picking for fillets), `bake/` (all bakes).
- `crates/forme-render`: WGSL raymarcher (`sdf.wgsl`), preview of the baked mesh (`mesh.rs`), headless capture.
- `crates/forme-app`: two desktop interfaces chosen in Settings: Atelier (`ui.rs`, floating panels, also the iPad layout) and Studio (`studio.rs`, docked panels and menus, 3ds Max style); user preferences (`settings.rs`, `settings.json` next to the projects, `FORME_UI=studio|atelier` overrides); gestures (`viewport.rs`), state and tools (`state.rs`), frame composition and progressive rendering (`gfx.rs`), entry point (`main.rs`), kit of parts (`kit.rs`), capture harness (`bin/shot.rs`).

## Rules that are easy to break

- **The shader (`sdf.wgsl`), the f32 CPU interpreter (`program.rs::eval_prim`) and the f64 bake interpreter (`bake/field.rs::prim_raw`) must stay identical.** Any change to a primitive (for example per-edge fillets) goes in all three.
- Box edge indices (12): `axis*4 + k`, with k over the signs of the other two axes `(+,+) (-,+) (-,-) (+,-)`, other axes = `(axis+1)%3, (axis+2)%3`. Cylinder: 0 = top rim, 1 = bottom rim.
- `Shape.edges` empty = uniform fillet (`round` / `chamfer`). Negative value = chamfer. `set_edge` collapses back to uniform when every edge is equal.
- Bake build order (validated by Romain): **fillets first**, as quad strips shown in orange in debug renders, **then the fill paths** (flat faces, walls) between them, reusing the strips' edge vertices exactly. See `docs/BAKE.md`.
- The flat-loop pass (`bake/decimate.rs`) is an **option, off by default** (validated by Romain): never call it from `bake()`. It runs only when the toggle in the Bake window is on, on a copy of the result.
- The Bake tries, in order: by construction lines (`bake/layout.rs`), by operation (`bake/build.rs`), by surfaces. Each step returns `Err(reason)` for a history it does not handle; the reason shows in the Bake panel.
- Bake by operation (`bake/build.rs`): any history it does not handle must return `Err(reason)`. `bake()` then falls back to the surface bake and shows the reason in the dialog. Never return a broken mesh in silence.
- Renderer: an image is "moving" when it changes from frame to frame (not just because a tool is active). The full-quality image is refined in bands, so it never blocks the UI.
- Both interfaces drive the same `AppState`: a feature goes in `state.rs`, then gets a control in `ui.rs` (Atelier) and in `studio.rs` (Studio). Each interface sets `st.view_rect` every frame (the free 3D area), which centres the camera.
- Features hidden by the basic mode (`Settings.advanced` off) are hidden, not removed: check `st.settings.prim_allowed()` / `modes()` before showing them.
- Never commit `target/` or `shots/replay/frames/`.
