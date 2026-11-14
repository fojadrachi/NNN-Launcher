//! App-Updates über GitHub-Releases: neueste Version prüfen, Setup laden, SHA-256 prüfen,
//! still installieren. Das Setup startet den Launcher danach neu.

use crate::{net, paths};
use anyhow::{bail, Result};
use sha2::{Digest, Sha256};

const LATEST: &str = "https://api.github.com/repos/fojadrachi/NNN-Launcher/releases/latest";
const SETUP_PREFIX: &str = "NNN-Launcher-Setup-";
/// Linux-Paket: NNN-Launcher-<Version>-linux-x64.tar.gz
const LINUX_SUFFIX: &str = "-linux-x64.tar.gz";

fn is_update_asset(name: &str) -> bool {
    if cfg!(windows) {
        name.starts_with(SETUP_PREFIX) && name.to_lowercase().ends_with(".exe")
    } else {
        name.starts_with("NNN-Launcher-") && name.ends_with(LINUX_SUFFIX)
    }
}

#[derive(Clone, Debug, Default)]
pub struct UpdateInfo {
    pub available: bool,
    pub latest: String,
    pub notes: String,
    pub url: String,
    pub size: u64,
    pub sha256: Option<String>,
}

fn parse(v: &str) -> (u64, u64, u64) {
    let mut it = v.trim_start_matches(['v', 'V']).split(['.', '-', '+']).map(|p| p.parse().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

pub async fn check() -> Result<UpdateInfo> {
    let res = net::client().get(LATEST).header("Accept", "application/vnd.github+json").send().await?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(UpdateInfo::default());
    }
    let rel: serde_json::Value = res.error_for_status()?.json().await?;
    let tag = rel["tag_name"].as_str().unwrap_or_default().to_string();
    let asset = rel["assets"].as_array().into_iter().flatten().find(|a| is_update_asset(a["name"].as_str().unwrap_or_default()));
    let newer = parse(&tag) > parse(net::VERSION);
    Ok(UpdateInfo {
        available: newer && asset.is_some(),
        latest: tag.trim_start_matches(['v', 'V']).to_string(),
        notes: rel["body"].as_str().unwrap_or_default().to_string(),
        url: asset.and_then(|a| a["browser_download_url"].as_str()).unwrap_or_default().to_string(),
        size: asset.and_then(|a| a["size"].as_u64()).unwrap_or(0),
        sha256: asset.and_then(|a| a["digest"].as_str()).and_then(|d| d.strip_prefix("sha256:")).map(|s| s.to_lowercase()),
    })
}

/// Lädt das Setup, prüft es und startet die stille Installation. Danach muss sich die App beenden.
pub async fn install(info: &UpdateInfo) -> Result<()> {
    if !info.available || !info.url.starts_with("https://github.com/") {
        bail!("Kein gültiges Update verfügbar");
    }
    let name = if cfg!(windows) { format!("{SETUP_PREFIX}{}.exe", info.latest) } else { format!("NNN-Launcher-{}{LINUX_SUFFIX}", info.latest) };
    let file = paths::cache().join("updates").join(name);
    let _ = std::fs::remove_file(&file);
    net::download_one(&net::DlItem { url: info.url.clone(), path: file.clone(), sha1: None, size: info.size }).await?;
    if let Some(expected) = &info.sha256 {
        let got = hex::encode(Sha256::digest(std::fs::read(&file)?));
        if &got != expected {
            let _ = std::fs::remove_file(&file);
            bail!("Prüfsumme des Updates stimmt nicht – abgebrochen.");
        }
    }
    apply(&file)
}

#[cfg(windows)]
fn apply(setup: &std::path::Path) -> Result<()> {
    std::process::Command::new(setup).args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS"]).spawn()?;
    Ok(())
}

/// Linux: neues Programm aus dem Paket holen, die laufende Datei ersetzen (das darf man unter Linux)
/// und neu starten
#[cfg(not(windows))]
fn apply(archive: &std::path::Path) -> Result<()> {
    let dir = archive.with_extension("").with_extension("");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let ok = std::process::Command::new("tar").arg("-xzf").arg(archive).arg("-C").arg(&dir).status().map(|s| s.success()).unwrap_or(false);
    let new = std::fs::read_dir(&dir)?.flatten().map(|e| e.path().join("nnn-launcher")).find(|p| p.is_file()).or_else(|| Some(dir.join("nnn-launcher")).filter(|p| p.is_file()));
    let Some(new) = new.filter(|_| ok) else { bail!("Update-Paket ist unvollständig") };
    let exe = std::env::current_exe()?;
    let tmp = exe.with_extension("new");
    std::fs::copy(&new, &tmp)?;
    crate::platform::make_executable(&tmp);
    std::fs::rename(&tmp, &exe)?;
    let _ = std::fs::remove_dir_all(&dir);
    std::process::Command::new(&exe).spawn()?;
    Ok(())
}

#[cfg(not(windows))]
pub fn is_installed() -> bool { true }

#[cfg(windows)]
pub fn is_installed() -> bool {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_lowercase().starts_with(&format!("{}\\programs", local.to_lowercase())))
        .unwrap_or(false)
}
