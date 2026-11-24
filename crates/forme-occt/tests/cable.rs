//! A cable: a disc swept along a smooth curve (24 nov.).
use forme_occt::Shape;

#[test]
fn cable_swept_and_capped() {
    let pts = [[0.0, 2.0, 0.0], [20.0, 8.0, 0.0], [40.0, 2.0, 10.0], [60.0, 5.0, 10.0]];
    let s = Shape::cable(&pts, 1.5).expect("a cable");
    let v = s.volume();
    // about π r² × its length (a little over 70 mm)
    let a = std::f64::consts::PI * 1.5 * 1.5;
    assert!(v > a * 60.0 && v < a * 90.0, "volume {v}");
    // two points: a straight rod
    let r = Shape::cable(&[[0.0, 0.0, 0.0], [0.0, 0.0, 30.0]], 2.0).expect("a rod");
    let v = r.volume();
    assert!((v - std::f64::consts::PI * 4.0 * 30.0).abs() < 1.0, "rod volume {v}");
    // a turn tighter than its radius: refused
    let bad = Shape::cable(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]], 3.0);
    assert!(bad.is_err());
}
