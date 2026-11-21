//! Version shown in the window title: branch, commit and commit date, read
//! from git at build time (empty when built outside a git checkout). On
//! Windows, the app icon (`assets/forme-icon.ico`) goes into the .exe.

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
    println!("cargo:rerun-if-changed=assets/forme-icon.ico");
    // the .exe's icon (Explorer, taskbar, shortcuts): never fails the build
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/forme-icon.ico");
        res.set("ProductName", "Forme");
        res.set("FileDescription", "Forme");
        if let Err(e) = res.compile() {
            println!("cargo:warning=app icon not embedded in the .exe: {e}");
        }
    }
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let branch = branch.strip_prefix("claude/").unwrap_or(&branch);
    let commit = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_default();
    let date = git(&["log", "-1", "--format=%cd", "--date=format-local:%d/%m/%Y %H:%M"]).unwrap_or_default();
    println!("cargo:rustc-env=FORME_GIT_BRANCH={branch}");
    println!("cargo:rustc-env=FORME_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=FORME_GIT_DATE={date}");
}
