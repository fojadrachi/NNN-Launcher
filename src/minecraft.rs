//! Minecraft-Versionsliste und Java-Laufzeiten von Mojang.
//! Spieldateien selbst lädt der offizielle Launcher; Java brauchen wir nur für die Forge-Installer.

use crate::{net, paths};
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::path::PathBuf;
use tokio::sync::OnceCell;

const VERSION_MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const JAVA_RUNTIMES: &str =
    "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";

static MANIFEST: OnceCell<Value> = OnceCell::const_new();

async fn manifest() -> Result<&'static Value> {
    MANIFEST
        .get_or_try_init(|| async {
            let cache = paths::cache().join("version_manifest_v2.json");
            match net::get_text(VERSION_MANIFEST).await {
                Ok(text) => {
                    let _ = std::fs::write(&cache, &text);
                    Ok(serde_json::from_str(&text)?)
                }
                // Offline: letzte bekannte Liste
                Err(e) => std::fs::read_to_string(&cache)
                    .ok()
                    .and_then(|t| serde_json::from_str(&t).ok())
                    .ok_or(e),
            }
        })
        .await
}

/// Release-Versionen, neueste zuerst
pub async fn release_versions() -> Result<Vec<String>> {
    let m = manifest().await?;
    Ok(m["versions"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|v| v["type"] == "release")
                .filter_map(|v| v["id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default())
}

async fn version_json(id: &str) -> Result<Value> {
    let m = manifest().await?;
    let entry = m["versions"]
        .as_array()
        .and_then(|a| a.iter().find(|v| v["id"] == id))
        .ok_or_else(|| anyhow!("Minecraft-Version {id} nicht gefunden"))?;
    net::get_json(entry["url"].as_str().unwrap_or_default()).await
}

/// Passende Java-Laufzeit für eine Minecraft-Version (für die Forge/NeoForge-Installer)
pub async fn java_for(mc: &str, progress: &(dyn Fn(String) + Send + Sync)) -> Result<PathBuf> {
    let v = version_json(mc).await?;
    let component = v["javaVersion"]["component"].as_str().unwrap_or("jre-legacy").to_string();
    ensure_java(&component, progress).await
}

pub async fn ensure_java(component: &str, progress: &(dyn Fn(String) + Send + Sync)) -> Result<PathBuf> {
    let dir = paths::runtimes().join(component);
    let java = dir.join("bin").join("java.exe");
    let marker = dir.join(".packlink-ok");
    if java.exists() && marker.exists() {
        return Ok(java);
    }
    progress(format!("Java ({component}) herunterladen"));
    let all = net::get_json(JAVA_RUNTIMES).await?;
    let entry = all["windows-x64"][component]
        .as_array()
        .and_then(|a| a.first())
        .ok_or_else(|| anyhow!("Keine Java-Laufzeit '{component}' verfügbar"))?;
    let manifest = net::get_json(entry["manifest"]["url"].as_str().unwrap_or_default()).await?;

    let mut items = Vec::new();
    for (path, file) in manifest["files"].as_object().into_iter().flatten() {
        let target = paths::safe_join(&dir, path)?;
        match file["type"].as_str() {
            Some("directory") => std::fs::create_dir_all(&target)?,
            Some("file") => items.push(net::DlItem {
                url: file["downloads"]["raw"]["url"].as_str().unwrap_or_default().to_string(),
                path: target,
                sha1: file["downloads"]["raw"]["sha1"].as_str().map(String::from),
                size: file["downloads"]["raw"]["size"].as_u64().unwrap_or(0),
            }),
            _ => {}
        }
    }
    net::download_all(items, 12, |_, _| {}).await.context("Java-Download")?;
    if !java.exists() {
        anyhow::bail!("java.exe nach dem Download nicht gefunden");
    }
    std::fs::write(&marker, chrono::Utc::now().to_rfc3339())?;
    Ok(java)
}
