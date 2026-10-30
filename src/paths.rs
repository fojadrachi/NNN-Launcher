//! Ordner des Launchers. Gleiche Struktur wie die frühere C#-Version, damit Instanzen,
//! Anmeldung und heruntergeladene Dateien direkt weiter funktionieren.

use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

static ROOT: OnceLock<PathBuf> = OnceLock::new();

pub fn root() -> &'static Path {
    ROOT.get_or_init(|| {
        let appdata = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        let root = appdata.join("NNN Launcher");
        let old = appdata.join("PackLink");
        // Sehr alte Installationen hießen PackLink: einmalig übernehmen
        if !root.exists() && old.exists() && std::fs::rename(&old, &root).is_err() {
            return old;
        }
        root
    })
}

pub fn game() -> PathBuf { root().join("minecraft") }
pub fn versions() -> PathBuf { game().join("versions") }
pub fn libraries() -> PathBuf { game().join("libraries") }
pub fn assets() -> PathBuf { game().join("assets") }
pub fn natives() -> PathBuf { root().join("natives") }
pub fn runtimes() -> PathBuf { root().join("runtimes") }
pub fn cache() -> PathBuf { root().join("cache") }
pub fn instances() -> PathBuf { root().join("instances") }
pub fn logs() -> PathBuf { root().join("logs") }

pub fn ensure() {
    for d in [root().to_path_buf(), game(), versions(), runtimes(), cache(), instances(), logs()] {
        let _ = std::fs::create_dir_all(d);
    }
}

/// Hängt einen relativen Pfad aus einem Manifest an, ohne aus `base` ausbrechen zu können.
pub fn safe_join(base: &Path, relative: &str) -> anyhow::Result<PathBuf> {
    let rel = Path::new(relative);
    let mut out = base.to_path_buf();
    for c in rel.components() {
        match c {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            _ => anyhow::bail!("Unsicherer Pfad: {relative}"),
        }
    }
    Ok(out)
}
