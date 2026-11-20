//! Mesh export: OBJ (with vertex colours = material IDs) and binary glTF (.glb).
//! Material IDs travel as vertex colours so Substance Painter can bake an ID map
//! from a single texture set.

use crate::mesh::Mesh;
use crate::scene::Material;
use std::fmt::Write as _;

fn id_color(materials: &[Material], m: u32) -> [f32; 3] {
    materials.get(m as usize).map(|m| m.color).unwrap_or([0.6, 0.6, 0.6])
}

/// Wavefront OBJ with `v x y z r g b` vertex colours. Positions in mm.
pub fn to_obj(mesh: &Mesh, materials: &[Material], name: &str) -> String {
    let mut s = String::with_capacity(mesh.positions.len() * 80);
    let _ = writeln!(s, "# Forme export — {} triangles, unités : mm", mesh.triangle_count());
    let _ = writeln!(s, "o {}", name);
    for (p, m) in mesh.positions.iter().zip(&mesh.materials) {
        let c = id_color(materials, *m);
        let _ = writeln!(s, "v {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}", p.x, p.y, p.z, c[0], c[1], c[2]);
    }
    for n in &mesh.normals {
        let _ = writeln!(s, "vn {:.4} {:.4} {:.4}", n.x, n.y, n.z);
    }
    for t in mesh.indices.chunks(3) {
        let (a, b, c) = (t[0] + 1, t[1] + 1, t[2] + 1);
        let _ = writeln!(s, "f {a}//{a} {b}//{b} {c}//{c}");
    }
    s
}

fn pad4(v: &mut Vec<u8>, byte: u8) {
    while v.len() % 4 != 0 {
        v.push(byte);
    }
}

/// Binary glTF 2.0: one mesh, one material, COLOR_0 = material ID colours.
/// Positions are converted to metres (glTF convention).
pub fn to_glb(mesh: &Mesh, materials: &[Material], name: &str) -> Vec<u8> {
    let mut bin: Vec<u8> = vec![];
    let n = mesh.positions.len();
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in &mesh.positions {
        let q = [p.x * 0.001, p.y * 0.001, p.z * 0.001];
        for a in 0..3 {
            lo[a] = lo[a].min(q[a]);
            hi[a] = hi[a].max(q[a]);
        }
        for v in q {
            bin.extend_from_slice(&v.to_le_bytes());
        }
    }
    let pos_len = bin.len();
    let nrm_off = bin.len();
    for nrm in &mesh.normals {
        for v in [nrm.x, nrm.y, nrm.z] {
            bin.extend_from_slice(&v.to_le_bytes());
        }
    }
    let nrm_len = bin.len() - nrm_off;
    let col_off = bin.len();
    for m in &mesh.materials {
        let c = id_color(materials, *m);
        for v in [c[0], c[1], c[2], 1.0f32] {
            bin.extend_from_slice(&v.to_le_bytes());
        }
    }
    let col_len = bin.len() - col_off;
    let idx_off = bin.len();
    for i in &mesh.indices {
        bin.extend_from_slice(&i.to_le_bytes());
    }
    let idx_len = bin.len() - idx_off;
    pad4(&mut bin, 0);

    let json = format!(
        concat!(
            r#"{{"asset":{{"version":"2.0","generator":"Forme"}},"scene":0,"scenes":[{{"nodes":[0]}}],"#,
            r#""nodes":[{{"mesh":0,"name":"{name}"}}],"#,
            r#""meshes":[{{"name":"{name}","primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1,"COLOR_0":2}},"indices":3,"material":0}}]}}],"#,
            r#""materials":[{{"name":"Forme","pbrMetallicRoughness":{{"baseColorFactor":[1,1,1,1],"metallicFactor":0,"roughnessFactor":0.6}}}}],"#,
            r#""buffers":[{{"byteLength":{blen}}}],"#,
            r#""bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":{pl},"target":34962}},{{"buffer":0,"byteOffset":{no},"byteLength":{nl},"target":34962}},{{"buffer":0,"byteOffset":{co},"byteLength":{cl},"target":34962}},{{"buffer":0,"byteOffset":{io},"byteLength":{il},"target":34963}}],"#,
            r#""accessors":[{{"bufferView":0,"componentType":5126,"count":{n},"type":"VEC3","min":[{l0},{l1},{l2}],"max":[{h0},{h1},{h2}]}},"#,
            r#"{{"bufferView":1,"componentType":5126,"count":{n},"type":"VEC3"}},"#,
            r#"{{"bufferView":2,"componentType":5126,"count":{n},"type":"VEC4"}},"#,
            r#"{{"bufferView":3,"componentType":5125,"count":{ic},"type":"SCALAR"}}]}}"#
        ),
        name = name.replace('"', ""),
        blen = bin.len(),
        pl = pos_len,
        no = nrm_off,
        nl = nrm_len,
        co = col_off,
        cl = col_len,
        io = idx_off,
        il = idx_len,
        n = n,
        ic = mesh.indices.len(),
        l0 = if n > 0 { lo[0] } else { 0.0 },
        l1 = if n > 0 { lo[1] } else { 0.0 },
        l2 = if n > 0 { lo[2] } else { 0.0 },
        h0 = if n > 0 { hi[0] } else { 0.0 },
        h1 = if n > 0 { hi[1] } else { 0.0 },
        h2 = if n > 0 { hi[2] } else { 0.0 },
    );
    let mut json_bytes = json.into_bytes();
    pad4(&mut json_bytes, b' ');

    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}
