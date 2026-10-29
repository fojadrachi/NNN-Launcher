//! Fabric, Quilt, Forge und NeoForge: Versionslisten und Installation in den
//! gemeinsamen Minecraft-Ordner (wie .minecraft aufgebaut, den der offizielle Launcher nutzt).

use crate::{minecraft, net, paths};
use anyhow::{anyhow, bail, Result};
use std::io::Read;

pub const LOADERS: [&str; 5] = ["vanilla", "fabric", "quilt", "forge", "neoforge"];
pub const LOADER_NAMES: [&str; 5] = ["Vanilla", "Fabric", "Quilt", "Forge", "NeoForge"];

pub fn display(loader: &str) -> &'static str {
    LOADERS.iter().position(|l| *l == loader).map(|i| LOADER_NAMES[i]).unwrap_or("?")
}

#[derive(Clone, Debug)]
pub struct LoaderVersion {
    pub version: String,
    pub label: String,
}

pub async fn list(loader: &str, mc: &str) -> Result<Vec<LoaderVersion>> {
    let mk = |v: String, tag: &str| LoaderVersion { label: if tag.is_empty() { v.clone() } else { format!("{v} ({tag})") }, version: v };
    Ok(match loader {
        "fabric" => net::get_json(&format!("https://meta.fabricmc.net/v2/versions/loader/{mc}"))
            .await?
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| {
                let v = e["loader"]["version"].as_str()?.to_string();
                let stable = e["loader"]["stable"].as_bool().unwrap_or(false);
                Some(mk(v, if stable { "" } else { "beta" }))
            })
            .collect(),
        "quilt" => net::get_json(&format!("https://meta.quiltmc.org/v3/versions/loader/{mc}"))
            .await?
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| {
                let v = e["loader"]["version"].as_str()?.to_string();
                let beta = v.contains('-');
                Some(mk(v, if beta { "beta" } else { "" }))
            })
            .collect(),
        "forge" => {
            let xml = net::get_text("https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml").await?;
            let promos = net::get_json("https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json").await.ok();
            let rec = promos.as_ref().and_then(|p| p["promos"][format!("{mc}-recommended")].as_str().map(String::from));
            let prefix = format!("{mc}-");
            let re = regex::Regex::new(r"<version>([^<]+)</version>").unwrap();
            let mut list: Vec<LoaderVersion> = re
                .captures_iter(&xml)
                .filter_map(|c| c[1].strip_prefix(&prefix).map(String::from))
                .map(|v| {
                    let tag = if Some(&v) == rec.as_ref() { "empfohlen" } else { "" };
                    mk(v, tag)
                })
                .collect();
            // Empfohlene Version nach vorne
            if let Some(i) = list.iter().position(|l| l.label.contains("empfohlen")) {
                let r = list.remove(i);
                list.insert(0, r);
            }
            list
        }
        "neoforge" => {
            if mc == "1.20.1" {
                let d = net::get_json("https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/forge").await?;
                let mut v: Vec<LoaderVersion> = d["versions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str()?.strip_prefix("1.20.1-").map(|s| mk(s.to_string(), "")))
                    .collect();
                v.reverse();
                v
            } else {
                let p: Vec<&str> = mc.split('.').collect();
                let prefix = format!("{}.{}.", p.get(1).unwrap_or(&"0"), p.get(2).unwrap_or(&"0"));
                let d = net::get_json("https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge").await?;
                let mut v: Vec<LoaderVersion> = d["versions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str())
                    .filter(|v| v.starts_with(&prefix))
                    .map(|v| mk(v.to_string(), if v.contains("beta") { "beta" } else { "" }))
                    .collect();
                v.reverse();
                v
            }
        }
        _ => vec![],
    })
}

/// Installiert den Loader und gibt die Versions-ID für das Launcher-Profil zurück.
pub async fn install(loader: &str, mc: &str, version: Option<&str>, progress: &(dyn Fn(String) + Send + Sync)) -> Result<String> {
    if loader == "vanilla" || loader.is_empty() {
        return Ok(mc.to_string());
    }
    let v = version.filter(|v| !v.is_empty()).ok_or_else(|| anyhow!("Keine Loader-Version gewählt"))?;
    match loader {
        "fabric" => {
            install_profile(&format!("fabric-loader-{v}-{mc}"), &format!("https://meta.fabricmc.net/v2/versions/loader/{mc}/{v}/profile/json")).await
        }
        "quilt" => {
            install_profile(&format!("quilt-loader-{v}-{mc}"), &format!("https://meta.quiltmc.org/v3/versions/loader/{mc}/{v}/profile/json")).await
        }
        "forge" => {
            let url = format!("https://maven.minecraftforge.net/net/minecraftforge/forge/{mc}-{v}/forge-{mc}-{v}-installer.jar");
            install_forge_like(&url, mc, progress).await
        }
        "neoforge" => {
            let url = if mc == "1.20.1" {
                format!("https://maven.neoforged.net/releases/net/neoforged/forge/1.20.1-{v}/forge-1.20.1-{v}-installer.jar")
            } else {
                format!("https://maven.neoforged.net/releases/net/neoforged/neoforge/{v}/neoforge-{v}-installer.jar")
            };
            install_forge_like(&url, mc, progress).await
        }
        _ => bail!("Unbekannter Loader: {loader}"),
    }
}

async fn install_profile(id: &str, url: &str) -> Result<String> {
    let path = paths::versions().join(id).join(format!("{id}.json"));
    if path.exists() {
        return Ok(id.to_string());
    }
    let mut json = net::get_json(url).await?;
    json["id"] = serde_json::Value::String(id.to_string());
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, serde_json::to_vec(&json)?)?;
    Ok(id.to_string())
}

async fn install_forge_like(url: &str, mc: &str, progress: &(dyn Fn(String) + Send + Sync)) -> Result<String> {
    let name = url.rsplit('/').next().unwrap_or("installer.jar");
    let installer = paths::cache().join("installers").join(name);
    if !installer.exists() {
        progress("Loader-Installer herunterladen".into());
        net::download_one(&net::DlItem { url: url.into(), path: installer.clone(), sha1: None, size: 0 }).await?;
    }

    // Versions-ID steht in version.json im Installer
    let id = {
        let file = std::fs::File::open(&installer)?;
        let mut zip = zip::ZipArchive::new(file)?;
        let mut entry = zip.by_name("version.json").map_err(|_| anyhow!("Installer enthält keine version.json"))?;
        let mut s = String::new();
        entry.read_to_string(&mut s)?;
        serde_json::from_str::<serde_json::Value>(&s)?["id"].as_str().unwrap_or_default().to_string()
    };
    let dir = paths::versions().join(&id);
    let marker = dir.join(".packlink-installed");
    if dir.join(format!("{id}.json")).exists() && marker.exists() {
        return Ok(id);
    }

    let java = minecraft::java_for(mc, progress).await?;
    let profiles = paths::game().join("launcher_profiles.json");
    if !profiles.exists() {
        std::fs::write(&profiles, r#"{"profiles":{}}"#)?;
    }

    progress("Loader installieren (kann ein paar Minuten dauern)".into());
    // Forge nutzt --installClient, NeoForge --install-client
    let (mut ok, mut out) = run_java(&java, &["-jar", &installer.to_string_lossy(), "--installClient", &paths::game().to_string_lossy()]).await?;
    if !ok && out.contains("installClient") {
        (ok, out) = run_java(&java, &["-jar", &installer.to_string_lossy(), "--install-client", &paths::game().to_string_lossy()]).await?;
    }
    let _ = std::fs::write(paths::logs().join("loader-install.log"), &out);
    if !ok || !dir.join(format!("{id}.json")).exists() {
        bail!("Loader-Installation fehlgeschlagen – Details in logs\\loader-install.log");
    }
    std::fs::write(&marker, chrono::Utc::now().to_rfc3339())?;
    Ok(id)
}

async fn run_java(java: &std::path::Path, args: &[&str]) -> Result<(bool, String)> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = tokio::process::Command::new(java);
    cmd.args(args).current_dir(paths::cache()).creation_flags(CREATE_NO_WINDOW);
    let out = cmd.output().await?;
    let text = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), text))
}
