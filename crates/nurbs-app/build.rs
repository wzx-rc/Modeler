//! Version shown in the window title: branch, commit and commit date, read
//! from git at build time (empty when built outside a git checkout).

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn main() {
    // rebuild when HEAD moves (checkout, commit, pull)
    if let Some(dir) = git(&["rev-parse", "--git-dir"]) {
        let dir = std::path::PathBuf::from(dir);
        for f in ["HEAD", "refs", "packed-refs"] {
            println!("cargo:rerun-if-changed={}", dir.join(f).display());
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let branch = branch.strip_prefix("claude/").unwrap_or(&branch);
    let commit = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_default();
    let date = git(&["log", "-1", "--format=%cd", "--date=format-local:%d/%m/%Y %H:%M"]).unwrap_or_default();
    println!("cargo:rustc-env=FORME_GIT_BRANCH={branch}");
    println!("cargo:rustc-env=FORME_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=FORME_GIT_DATE={date}");
}
