//! Runs the OpenCascade kernel process (`forme-occ`, crate nurbs-occ) on a
//! scene. OpenCascade aborts the process on some failures (C++ exceptions
//! the binding does not catch): the last `STEP <what>:<id>` line on stderr
//! says where, and the scene is rebuilt with that step skipped, so a fillet
//! that cannot be made leaves its edges sharp instead of losing the part.

use crate::NurbsModel;
use forme_core::scene::Scene;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The kernel process: `FORME_OCC` if set, else `forme-occ` next to the
/// running executable. None when it is not there (the app falls back to
/// the pure Rust kernel).
pub fn occ_exe() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FORME_OCC") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = format!("forme-occ{}", std::env::consts::EXE_SUFFIX);
    // next to the app, or one level up (cargo puts examples and tests in sub folders)
    [dir.join(&name), dir.parent()?.join(&name)].into_iter().find(|p| p.is_file())
}

/// How many steps may be skipped before giving up.
const MAX_SKIPS: usize = 12;

/// One run of the process. Ok(result) when it finished, Err(last step) when
/// it aborted.
fn run_once(exe: &Path, json: &[u8], skip: &[String]) -> Result<Result<NurbsModel, String>, Option<String>> {
    let mut cmd = Command::new(exe);
    if !skip.is_empty() {
        cmd.arg("--skip").arg(skip.join(","));
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| Some(format!("start: {e}")))?;
    // the process reads all of stdin before writing anything
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(json);
    }
    let out = child.wait_with_output().map_err(|e| Some(format!("wait: {e}")))?;
    if out.status.success() {
        if let Some(r) = crate::proto::decode(&out.stdout) {
            return Ok(r);
        }
    }
    let err = String::from_utf8_lossy(&out.stderr);
    Err(err.lines().rev().find_map(|l| l.strip_prefix("STEP ")).map(|s| s.trim().to_string()))
}

/// Builds the scene with OpenCascade. Skips the step that crashed the
/// kernel (a fillet, a rounding, one boolean) and tries again.
pub fn build_occ(exe: &Path, scene: &Scene) -> Result<NurbsModel, String> {
    let json = serde_json::to_vec(scene).map_err(|e| format!("scene: {e}"))?;
    let mut skip: Vec<String> = vec![];
    loop {
        match run_once(exe, &json, &skip) {
            Ok(r) => return r,
            Err(step) => {
                let Some(step) = step else { return Err("the NURBS kernel crashed".into()) };
                let skippable = ["fillet:", "round:", "union:", "cut:"].iter().any(|p| step.starts_with(p));
                if !skippable || skip.contains(&step) || skip.len() >= MAX_SKIPS {
                    return Err(format!("the NURBS kernel crashed ({step})"));
                }
                skip.push(step);
            }
        }
    }
}
