//! Documents (.fnurbs, JSON) and settings live in Documents/Forme NURBS.

use nurbs_kernel::Doc;
use std::path::{Path, PathBuf};

pub const EXT: &str = "fnurbs";

pub fn documents_dir() -> PathBuf {
    let dir = if let Ok(d) = std::env::var("FORME_DOCS") {
        PathBuf::from(d)
    } else if cfg!(target_os = "ios") {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join("Documents")
    } else {
        std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map(|h| PathBuf::from(h).join("Documents").join("Forme NURBS"))
            .unwrap_or_else(|_| PathBuf::from("forme-nurbs-docs"))
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn sanitize(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' }).collect();
    let s = s.trim().to_string();
    if s.is_empty() {
        "Untitled".into()
    } else {
        s
    }
}

pub fn path_of(name: &str) -> PathBuf {
    documents_dir().join(format!("{}.{EXT}", sanitize(name)))
}

pub fn save(doc: &Doc, name: &str) -> std::io::Result<PathBuf> {
    let p = path_of(name);
    // written aside then put in place: a crash while writing never leaves
    // half a document
    let tmp = p.with_extension(format!("{EXT}.tmp"));
    std::fs::write(&tmp, doc.to_json())?;
    std::fs::rename(&tmp, &p)?;
    Ok(p)
}

pub fn load(path: &Path) -> Result<Doc, String> {
    let s = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    Doc::from_json(&s)
}

/// Saved documents, most recent first (the session autosave left out).
pub fn list() -> Vec<(String, PathBuf)> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(documents_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == EXT).unwrap_or(false) {
                let name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                if name == SESSION {
                    continue;
                }
                let t = e.metadata().and_then(|m| m.modified()).ok();
                out.push((t, name, p));
            }
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.into_iter().map(|(_, n, p)| (n, p)).collect()
}

/// The work in progress, saved on quit and reopened at start.
pub const SESSION: &str = "Session";
