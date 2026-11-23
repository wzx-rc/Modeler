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

/// The kernel's last model of a document, kept beside the documents
/// (`cache/`, by a hash of its features) and shown at once when the same
/// document is opened again, while the kernel builds it (Romain, 23 nov.,
/// 22 h 05: "Quand j'ouvre la scene très longue à tout rebuild"). Whatever
/// the file it comes from: the session, a document saved, an undo.
pub fn model_key(doc: &Doc) -> u64 {
    let s = serde_json::to_string(&doc.features).unwrap_or_default();
    nurbs_kernel::doc::fnv(s.as_bytes())
}

fn model_path(key: u64) -> PathBuf {
    documents_dir().join("cache").join(format!("{key:016x}.model"))
}

/// How many models are kept (the most recent).
const MODELS_KEPT: usize = 16;

pub fn save_model(key: u64, m: &nurbs_kernel::Model) -> std::io::Result<()> {
    let p = model_path(key);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("model.tmp");
    std::fs::write(&tmp, m.encode())?;
    std::fs::rename(&tmp, &p)?;
    // the oldest beyond the count taken away
    if let Some(dir) = p.parent() {
        let mut all: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "model"))
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        all.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, q) in all.into_iter().skip(MODELS_KEPT) {
            let _ = std::fs::remove_file(q);
        }
    }
    Ok(())
}

pub fn load_model(key: u64) -> Option<nurbs_kernel::Model> {
    let b = std::fs::read(model_path(key)).ok()?;
    nurbs_kernel::Model::decode(&b)
}
