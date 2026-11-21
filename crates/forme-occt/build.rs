//! Builds (or finds) OpenCascade, compiles cpp/forme_occt.cpp against it and
//! links the OpenCascade libraries it uses.

use std::path::PathBuf;

/// OpenCascade toolkits used by forme_occt.cpp, dependents first (static
/// linking order).
const LIBS: &[&str] = &[
    "TKOffset", "TKFillet", "TKBool", "TKBO", "TKPrim", "TKShHealing", "TKMesh", "TKTopAlgo", "TKGeomAlgo", "TKBRep", "TKGeomBase", "TKG3d", "TKG2d", "TKMath", "TKernel",
];

fn occt_root() -> PathBuf {
    println!("cargo:rerun-if-env-changed=FORME_OCCT_ROOT");
    if let Ok(p) = std::env::var("FORME_OCCT_ROOT") {
        return PathBuf::from(p);
    }
    #[cfg(feature = "builtin")]
    {
        occt_sys::build_occt();
        return occt_sys::occt_path();
    }
    #[allow(unreachable_code)]
    {
        panic!("OpenCascade not found: enable the `builtin` feature or set FORME_OCCT_ROOT")
    }
}

fn main() {
    let root = occt_root();
    let lib = root.join("lib");
    let inc = if root.join("include").join("opencascade").is_dir() { root.join("include").join("opencascade") } else { root.join("include") };
    println!("cargo:rerun-if-changed=cpp/forme_occt.cpp");
    // A crash inside OpenCascade (an access violation in a fillet) caught as
    // an exception, as OpenCascade itself is built: on Windows structured
    // exceptions (/EHa), elsewhere signals turned into jumps.
    let msvc = std::env::var("TARGET").unwrap_or_default().contains("msvc");
    let mut b = cc::Build::new();
    b.cpp(true)
        .file("cpp/forme_occt.cpp")
        .include(&inc)
        .flag_if_supported("-std=c++17")
        .flag_if_supported("/std:c++17")
        .flag_if_supported("-w")
        .define("_USE_MATH_DEFINES", None);
    if msvc {
        b.flag("/EHa");
    } else {
        b.define("OCC_CONVERT_SIGNALS", None);
    }
    b.compile("forme_occt");
    println!("cargo:rustc-link-search=native={}", lib.display());
    for l in LIBS {
        println!("cargo:rustc-link-lib=static={l}");
    }
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("windows") {
        println!("cargo:rustc-link-lib=dylib=user32");
        println!("cargo:rustc-link-lib=dylib=advapi32");
        println!("cargo:rustc-link-lib=dylib=ws2_32");
    }
}
