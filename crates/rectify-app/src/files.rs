//! Scans are read from Documents/Forme Rectify.

use std::path::PathBuf;

pub fn documents_dir() -> PathBuf {
    let dir = if let Ok(d) = std::env::var("FORME_DOCS") {
        PathBuf::from(d)
    } else {
        std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map(|h| PathBuf::from(h).join("Documents").join("Forme Rectify"))
            .unwrap_or_else(|_| PathBuf::from("forme-rectify-docs"))
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

/// The .obj files of the documents folder, newest first.
pub fn list() -> Vec<(String, PathBuf, u64)> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(documents_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x.eq_ignore_ascii_case("obj")).unwrap_or(false) {
                let meta = e.metadata().ok();
                let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                let t = meta.and_then(|m| m.modified().ok()).unwrap_or(std::time::UNIX_EPOCH);
                out.push((t, p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), p, size));
            }
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.into_iter().map(|(_, n, p, s)| (n, p, s)).collect()
}
