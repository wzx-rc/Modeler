//! Projects (.forme, JSON) and exports live in the app's Documents folder,
//! which iOS shows in the Files app (UIFileSharingEnabled).

use forme_core::Scene;
use std::path::{Path, PathBuf};

pub fn documents_dir() -> PathBuf {
    let dir = if let Ok(d) = std::env::var("FORME_DOCS") {
        PathBuf::from(d)
    } else if cfg!(target_os = "ios") {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join("Documents")
    } else {
        // Documents/Forme in the user's home (USERPROFILE on Windows)
        std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map(|h| PathBuf::from(h).join("Documents").join("Forme"))
            .unwrap_or_else(|_| PathBuf::from("forme-docs"))
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' })
        .collect();
    let s = s.trim().to_string();
    if s.is_empty() {
        "Sans titre".into()
    } else {
        s
    }
}

pub fn project_path(name: &str) -> PathBuf {
    documents_dir().join(format!("{}.forme", sanitize(name)))
}

pub fn save(scene: &Scene, name: &str) -> std::io::Result<PathBuf> {
    let path = project_path(name);
    std::fs::write(&path, scene.to_json())?;
    Ok(path)
}

pub fn load(path: &Path) -> Result<Scene, String> {
    let s = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    Scene::from_json(&s).map_err(|e| e.to_string())
}

/// Saved projects, most recent first.
pub fn list() -> Vec<(String, PathBuf)> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(documents_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "forme").unwrap_or(false) {
                let t = e.metadata().and_then(|m| m.modified()).ok();
                let name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                out.push((t, name, p));
            }
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.into_iter().map(|(_, n, p)| (n, p)).collect()
}
