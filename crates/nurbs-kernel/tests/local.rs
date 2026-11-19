//! The kernel built in the app (iPad): same answers as the process, and a
//! step written before a crash is skipped at the next start.
#![cfg(feature = "occ")]

use glam::{DQuat, DVec3};
use nurbs_kernel::client::Kernel;
use nurbs_kernel::{Doc, Op, Prim};
use std::time::Duration;

fn part() -> Doc {
    let mut d = Doc::default();
    d.add(Op::Add, Prim::Box { x: 40.0, y: 20.0, z: 30.0 }, DVec3::ZERO, DQuat::IDENTITY);
    d.add(Op::Carve, Prim::Cylinder { radius: 5.0, height: 30.0 }, DVec3::new(-10.0, -5.0, 0.0), DQuat::IDENTITY);
    d
}

#[test]
fn local_kernel_builds() {
    let dir = std::env::temp_dir().join(format!("forme-local-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let k = Kernel::start_local(&dir);
    assert!(k.available());
    k.submit(1, &part(), true, false);
    let b = k.wait_for(1, Duration::from_secs(60)).expect("an answer");
    assert!(b.model.errors.is_empty(), "{:?}", b.model.errors);
    assert!(b.model.faces.len() >= 7, "{} faces", b.model.faces.len());
    // built to the end: no step left to blame
    assert!(!dir.join("kernel-step.txt").exists());
    // a step left by a "crash" is skipped next time
    std::fs::write(dir.join("kernel-step.txt"), "boolean:2:0").unwrap();
    drop(k);
    let skip = {
        // start_local reads it (STEP_FILE is set once per process: only the
        // skip list is checked here)
        let k = Kernel::start_local(&dir);
        drop(k);
        std::fs::read_to_string(dir.join("kernel-skip.txt")).unwrap()
    };
    assert!(skip.lines().any(|l| l == "boolean:2:0"), "{skip}");
    let _ = std::fs::remove_dir_all(&dir);
}
