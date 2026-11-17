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
2. Selection of the real B-rep edges (from the kernel's topology: face tags and edge adjacency), fillets and chamfers.
3. Quick creation and stacking, workplanes, Sketch (extruded profiles), materials (6 balls).

## Commands

```bash
export FORME_OCCT_ROOT=/path/occt-7.8                        # an installed OCCT 7.8 (lib/ include/); on PC use --features builtin instead
cargo test --release -p forme-occt --no-default-features      # the C layer: cube − hole, fillet, face tags, topology
cargo test --release -p nurbs-kernel --features occ           # the document → B-rep build, caches, skip, encode / decode
cargo build --release -p nurbs-kernel --features occ          # the kernel process forme-nurbs-kernel
cargo build --release -p nurbs-kernel --features builtin      # same, building OpenCascade from source the first time (6 to 20 min, cmake + C++ compiler)
cargo build --release -p nurbs-app                            # the app forme-nurbs and the capture harness forme-nurbs-shot
cargo run --release -p nurbs-app --bin forme-nurbs            # the app (finds target/release/forme-nurbs-kernel next to it, or FORME_NURBS_KERNEL=path)
./target/release/forme-nurbs-shot /tmp/x                      # every capture scenario on the test part; prefix filter: empty, part, move, create
```

`forme-nurbs-shot` runs the real app headless (egui + wgpu, real kernel process, real mouse events). It accepts a software GPU (lavapipe / llvmpipe) and writes full-quality PNGs at 1600×900 points, scale 2. Use it to check every visual change before showing it.

The window title shows the version, branch, commit and commit date (`crates/nurbs-app/build.rs`, `nurbs_app::version_title()`): check it before saying which version Romain is running.

## Code map

- `crates/forme-occt`: our thin C layer over OpenCascade (`cpp/forme_occt.cpp` + safe Rust wrapper). Primitives, transforms, booleans, fillets / chamfers by edge index, meshing. Every OpenCascade exception is caught and returned as `Err`; only `opencascade::handle<T>` is used (the `Handle_T` aliases break under MSVC). **Face tags**: each face carries an `i64` tag that booleans, transforms, fillets and chamfers carry to the faces they make (`carry_tags`, history `Modified`); new fillet / chamfer faces get the `fresh` tag. `Shape::topo(defl, angle)` returns in one buffer the faces (tag, surface kind, plane point / normal, axis, radius), the mesh with a face index per triangle, and the edges (index, the two faces, kind, seam, polyline). OpenCascade is built by `occt-sys` (feature `builtin`, cached in `target/OCCT`) or taken from `FORME_OCCT_ROOT`.
- `crates/nurbs-kernel`: the document and the kernel.
  - `doc.rs`: `Doc { features }`, `Feature { id, name, op, prim, origin, rot, visible, material }`. A feature's origin is the centre of its base; it rises along its local +Y. Saved as JSON `.fnurbs`.
  - `model.rs`: what the kernel sends back: mesh, `tri_face`, faces with their `FaceTag { feature, sub }` (box sub: 0 +X, 1 −X, 2 top, 3 bottom, 4 +Z, 5 −Z; cylinder: 0 side, 1 top, 2 bottom), edges, errors; binary encode / decode.
  - `build.rs` (feature `occ`): features applied in order (a Carve with nothing before it is ignored), tool solids cached by feature hash, stages by prefix hash, `STEP what:id:hash` written on stderr before each risky step.
  - `client.rs`: the app side. The kernel runs in its own process `forme-nurbs-kernel` (u32-length frames on stdin / stdout, JSON request, binary model): a hard crash inside OpenCascade cannot take the app down; the client restarts it with the failing step skipped and the model lists what was left out. The latest request wins; coarse meshing while a gesture is live, fine after.
- `crates/nurbs-app`: the app.
  - `render.rs`: wgpu, MSAA 4×: background (gradient, ground grid 10 / 50 mm, origin axes), the part's mesh with per-face flags (selected / hovered) in a storage buffer, B-rep edges as anti-aliased screen-width lines, overlay outlines (cutters, shapes being moved or created) with a dimmed hidden part.
  - `pick.rs`: ray against the kernel's triangles, face by face → the face's tag → the feature. No SDF, no approximation.
  - `state.rs`: `AppState` (document, undo / redo with live edits, selection, tools, 3ds Max-like creation on the ground or a flat face, move gizmo and size handles, kernel round trips). Features go here first.
  - `viewport.rs`: gestures (Maya navigation: Alt + left orbit, Alt + middle pan, Alt + right zoom, wheel zoom), gizmo, creation and marquee drawing, kernel status line.
  - `ui.rs` + `widgets.rs` + `theme.rs` + `icons.rs`: the Établi-style islands, inspector, history strip, files dialog; `draw.rs`: what is uploaded to the renderer each frame; `files.rs`, `settings.rs`: `~/Documents/Forme NURBS`.
  - `bin/shot.rs`: the capture harness (`test_part()`: Block, Hole, Boss, Slot).

## Rules that are easy to break

- **No SDF, ever, on this branch**: the part on screen is always the kernel's mesh. While the kernel computes, the previous mesh stays, with the moved or created shape drawn as an outline.
- Picking and selection go through the kernel's faces and their tags (`Model::faces[f].tag`), never through a field or a guess from the shape list.
- A tag is `(feature << 16) | sub`; any new primitive must tag all its faces in `nurbs_kernel::build::sub_of`.
- Both sides of the kernel protocol (`proto.rs`, `model.rs` encode / decode) change together; the kernel process is rebuilt with the app.
- Never commit `target/` or `shots/replay/frames/`. Never run `cargo fmt` on the whole tree.
