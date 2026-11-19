//! Forme Rectify's shapes: outlines, discs, edges pulled out, thickness,
//! fillets on sheets and solids.

use forme_occt::{Shape, SurfKind};

fn square() -> Shape {
    let p = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 0.0, -10.0], [0.0, 0.0, -10.0]];
    let mut s = Shape::outline([0.0; 3], [0.0, 1.0, 0.0], &p, true, &[]).expect("square");
    s.set_tags(&[1]);
    s
}

#[test]
fn outlines() {
    let s = square();
    let t = s.topo(0.01, 0.2).unwrap();
    assert_eq!(t.faces.len(), 1);
    assert!(t.faces[0].info.normal[1] > 0.99, "{:?}", t.faces[0].info.normal);
    // the same points the other way round still face +y
    let p = [[0.0, 0.0, 0.0], [0.0, 0.0, -10.0], [10.0, 0.0, -10.0], [10.0, 0.0, 0.0]];
    let s2 = Shape::outline([0.0; 3], [0.0, 1.0, 0.0], &p, true, &[]).unwrap();
    assert!(s2.topo(0.01, 0.2).unwrap().faces[0].info.normal[1] > 0.99);
    // a rounded corner
    let cut = Some([[2.0, 0.0, 0.0], [0.5857864, 0.0, -0.5857864], [0.0, 0.0, -2.0]]);
    let s3 = Shape::outline([0.0; 3], [0.0, 1.0, 0.0], &p, true, &[cut, None, None, None]).unwrap();
    assert_eq!(s3.topo(0.01, 0.2).unwrap().edges.len(), 5);
    // an open line: free edges
    let l = Shape::outline([0.0; 3], [0.0, 1.0, 0.0], &p[..3], false, &[]).unwrap();
    let t = l.topo(0.01, 0.2).unwrap();
    assert_eq!(t.faces.len(), 0);
    assert_eq!(t.edges.len(), 2);
    // a crossing outline is refused
    let x = [[0.0, 0.0, 0.0], [10.0, 0.0, -10.0], [10.0, 0.0, 0.0], [0.0, 0.0, -10.0]];
    assert!(Shape::outline([0.0; 3], [0.0, 1.0, 0.0], &x, true, &[]).is_err());
}

#[test]
fn thick_and_round() {
    let s = square();
    let solid = s.thicken(2.0, 1).expect("thick");
    assert!((solid.volume() - 200.0).abs() < 1e-6, "{}", solid.volume());
    let t = solid.topo(0.01, 0.2).unwrap();
    assert_eq!(t.faces.len(), 6);
    let mut tags: Vec<i64> = t.faces.iter().map(|f| f.tag).collect();
    tags.sort();
    tags.dedup();
    assert_eq!(tags.len(), 6, "{tags:?}");
    assert!(t.faces.iter().any(|f| f.tag == 1 && f.info.normal[1] < -0.99));
    // the other way
    let down = s.thicken(-2.0, 1).expect("down");
    assert!((down.volume() - 200.0).abs() < 1e-6, "{}", down.volume());
    // a top edge rounded
    let e = t.edges.iter().find(|e| e.pts.iter().all(|p| (p[1] - 2.0).abs() < 1e-9 && p[2].abs() < 1e-9)).unwrap();
    let r = solid.round(&[e.index], 0.5, 99).expect("round");
    assert_eq!(r.topo(0.01, 0.2).unwrap().faces.len(), 7);
}

#[test]
fn edge_pulled_then_thick() {
    let s = square();
    let t = s.topo(0.01, 0.2).unwrap();
    let e = t.edges.iter().find(|e| e.pts.iter().all(|p| p[2].abs() < 1e-9)).unwrap();
    let sheet = s.extrude_edge(e.index, [0.0, 6.0, 0.0], 7).expect("pull");
    let t = sheet.topo(0.01, 0.2).unwrap();
    assert_eq!(t.faces.len(), 2);
    let shared: Vec<_> = t.edges.iter().filter(|e| e.faces[0] >= 0 && e.faces[1] >= 0).collect();
    assert_eq!(shared.len(), 1, "the fold joins the two faces");
    let tags: Vec<i64> = t.faces.iter().map(|f| f.tag).collect();
    assert!(tags.contains(&1) && tags.contains(&7), "{tags:?}");
    // the fold rounded on the sheet
    let rounded = sheet.round(&[shared[0].index], 1.0, 9).expect("fold fillet");
    let t = rounded.topo(0.01, 0.2).unwrap();
    assert_eq!(t.faces.len(), 3);
    assert!(t.faces.iter().any(|f| f.info.kind == SurfKind::Cylinder));
    // thickened
    let solid = rounded.thicken(0.8, 3).expect("thick sheet");
    let v = solid.volume();
    assert!(v > 0.0, "{v}");
    let t = solid.topo(0.01, 0.2).unwrap();
    assert!(t.edges.iter().all(|e| e.faces[0] >= 0 && e.faces[1] >= 0), "closed");
}

#[test]
fn line_pulled_and_disc() {
    let p = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 0.0, -10.0]];
    let l = Shape::outline([0.0; 3], [0.0, 1.0, 0.0], &p, false, &[]).unwrap();
    let t = l.topo(0.01, 0.2).unwrap();
    let e = t.edges[0].index;
    let s = l.extrude_edge(e, [0.0, 4.0, 0.0], 5).expect("pull a line");
    let t = s.topo(0.01, 0.2).unwrap();
    assert_eq!(t.faces.len(), 1);
    assert_eq!(t.edges.iter().filter(|e| e.faces[0] < 0 && e.faces[1] < 0).count(), 1, "the other side stays a line");
    let d = Shape::disc([0.0; 3], [0.0, 0.0, 1.0], 3.0).unwrap();
    let mut d = d;
    d.set_tags(&[1]);
    let c = d.thicken(5.0, 1).expect("cylinder");
    assert!((c.volume() - std::f64::consts::PI * 9.0 * 5.0).abs() < 1e-3);
    let t = c.topo(0.01, 0.2).unwrap();
    let rim = t.edges.iter().find(|e| !e.seam && e.pts.iter().all(|p| (p[2] - 5.0).abs() < 1e-9)).unwrap();
    let r = c.round(&[rim.index], 1.0, 9).expect("rim");
    assert_eq!(r.topo(0.01, 0.2).unwrap().faces.len(), 4);
}
