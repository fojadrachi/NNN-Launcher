//! App-Updates über GitHub-Releases: neueste Version prüfen, Setup laden, SHA-256 prüfen,
//! still installieren. Das Setup startet den Launcher danach neu.

use crate::{net, paths};
use anyhow::{bail, Result};
use sha2::{Digest, Sha256};

const LATEST: &str = "https://api.github.com/repos/fojadrachi/NNN-Launcher/releases/latest";
const SETUP_PREFIX: &str = "NNN-Launcher-Setup-";

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
    let asset = rel["assets"].as_array().into_iter().flatten().find(|a| {
        let n = a["name"].as_str().unwrap_or_default();
        n.starts_with(SETUP_PREFIX) && n.to_lowercase().ends_with(".exe")
    });
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
    let file = paths::cache().join("updates").join(format!("{SETUP_PREFIX}{}.exe", info.latest));
    let _ = std::fs::remove_file(&file);
    net::download_one(&net::DlItem { url: info.url.clone(), path: file.clone(), sha1: None, size: info.size }).await?;
    if let Some(expected) = &info.sha256 {
        let got = hex::encode(Sha256::digest(std::fs::read(&file)?));
        if &got != expected {
            let _ = std::fs::remove_file(&file);
            bail!("Prüfsumme des Updates stimmt nicht – abgebrochen.");
        }
    }
    std::process::Command::new(&file).args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS"]).spawn()?;
    Ok(())
}

pub fn is_installed() -> bool {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_lowercase().starts_with(&format!("{}\\programs", local.to_lowercase())))
        .unwrap_or(false)
}
