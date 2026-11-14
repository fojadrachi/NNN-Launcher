//! Start über den offiziellen Minecraft Launcher (wie bei CurseForge): Profil in
//! launcher_profiles.json schreiben und den Mojang-Launcher mit unserem Ordner öffnen.

use crate::instances::{self, Instance};
use crate::{loaders, net, paths};
use anyhow::{bail, Result};
use serde_json::{json, Map, Value};
use std::path::PathBuf;

#[cfg(windows)]
const DOWNLOAD_URL: &str = "https://launcher.mojang.com/download/Minecraft.exe";
#[cfg(not(windows))]
const DOWNLOAD_URL: &str = "https://launcher.mojang.com/download/Minecraft.tar.gz";

pub struct Launched {
    pub profile: String,
    pub already_running: bool,
}

pub async fn launch(inst: &Instance, memory_mb: u32, extra_args: &str, launcher_path: &str, progress: &(dyn Fn(String) + Send + Sync)) -> Result<Launched> {
    let m = &inst.manifest;
    progress("Loader vorbereiten".into());
    let version_id = loaders::install(&m.loader, &m.minecraft, m.loader_version.as_deref(), progress).await?;

    progress("Minecraft Launcher vorbereiten".into());
    let exe = find_or_download(launcher_path).await?;
    let name = format!("NNN: {}", inst.name);
    write_profile(&inst.id, &name, &version_id, inst.memory_mb.unwrap_or(memory_mb), extra_args)?;

    let already_running = launcher_running();
    // Selbsttest: alles vorbereiten, aber den Launcher nicht öffnen
    if std::env::var_os("NNN_DRY_LAUNCH").is_some() {
        return Ok(Launched { profile: format!("{name} ({version_id}, {})", exe.display()), already_running });
    }
    std::process::Command::new(&exe)
        .arg("--workDir")
        .arg(paths::game())
        .current_dir(exe.parent().unwrap_or(&paths::root().to_path_buf()))
        .spawn()?;

    let mut i = instances::get(&inst.id)?;
    i.last_played = instances::now();
    instances::save(&i)?;
    Ok(Launched { profile: name, already_running })
}

#[cfg(windows)]
fn launcher_running() -> bool { process_running("MinecraftLauncher.exe") || process_running("Minecraft.exe") }

#[cfg(not(windows))]
fn launcher_running() -> bool {
    std::process::Command::new("pgrep").args(["-f", "minecraft-launcher"]).output().map(|o| o.status.success()).unwrap_or(false)
}

#[cfg(windows)]
fn process_running(name: &str) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {name}"), "/NH"])
        .creation_flags(0x0800_0000)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase().contains(&name.to_lowercase()))
        .unwrap_or(false)
}

#[cfg(windows)]
async fn find_or_download(custom: &str) -> Result<PathBuf> {
    if !custom.trim().is_empty() {
        let p = PathBuf::from(custom.trim());
        if !p.exists() {
            bail!("Der eingestellte Minecraft Launcher wurde nicht gefunden: {}", p.display());
        }
        return Ok(p);
    }
    let bundled = paths::root().join("launcher").join("Minecraft.exe");
    let pf86 = std::env::var("ProgramFiles(x86)").unwrap_or_default();
    let pf = std::env::var("ProgramFiles").unwrap_or_default();
    for c in [
        PathBuf::from(&pf86).join("Minecraft Launcher").join("MinecraftLauncher.exe"),
        PathBuf::from(&pf).join("Minecraft Launcher").join("MinecraftLauncher.exe"),
        bundled.clone(),
    ] {
        if c.exists() {
            return Ok(c);
        }
    }
    net::download_one(&net::DlItem { url: DOWNLOAD_URL.into(), path: bundled.clone(), sha1: None, size: 0 }).await?;
    if !signed_by_mojang(&bundled) {
        let _ = std::fs::remove_file(&bundled);
        bail!("Heruntergeladener Minecraft Launcher ist nicht von Mojang/Microsoft signiert – abgebrochen.");
    }
    Ok(bundled)
}

/// Linux: installierten Launcher nutzen (Paket, PATH) oder Mojangs tar.gz einmalig entpacken
#[cfg(not(windows))]
async fn find_or_download(custom: &str) -> Result<PathBuf> {
    if !custom.trim().is_empty() {
        let p = PathBuf::from(custom.trim());
        if !p.exists() {
            bail!("Der eingestellte Minecraft Launcher wurde nicht gefunden: {}", p.display());
        }
        return Ok(p);
    }
    let dir = paths::root().join("launcher");
    let bundled = dir.join("minecraft-launcher").join("minecraft-launcher");
    let mut candidates = vec![bundled.clone(), PathBuf::from("/usr/bin/minecraft-launcher"), PathBuf::from("/opt/minecraft-launcher/minecraft-launcher")];
    if let Some(p) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&p).map(|d| d.join("minecraft-launcher")));
    }
    if let Some(c) = candidates.into_iter().find(|c| c.is_file()) {
        return Ok(c);
    }
    // Über HTTPS direkt von Mojang; unter Linux gibt es keine Authenticode-Signatur zum Prüfen
    let archive = dir.join("Minecraft.tar.gz");
    net::download_one(&net::DlItem { url: DOWNLOAD_URL.into(), path: archive.clone(), sha1: None, size: 0 }).await?;
    let ok = std::process::Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&dir).status().map(|s| s.success()).unwrap_or(false);
    let _ = std::fs::remove_file(&archive);
    if !ok || !bundled.is_file() {
        bail!("Minecraft Launcher konnte nicht entpackt werden (tar fehlt oder Download beschädigt)");
    }
    crate::platform::make_executable(&bundled);
    Ok(bundled)
}

/// Signatur über Windows prüfen (Get-AuthenticodeSignature)
#[cfg(windows)]
fn signed_by_mojang(file: &std::path::Path) -> bool {
    let script = format!(
        "$s = Get-AuthenticodeSignature -LiteralPath '{}'; if ($s.Status -eq 'Valid') {{ $s.SignerCertificate.Subject }}",
        file.display().to_string().replace('\'', "''")
    );
    crate::platform::quiet(std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]))
        .output()
        .map(|o| {
            let s = String::from_utf8_lossy(&o.stdout);
            s.contains("Mojang") || s.contains("Microsoft")
        })
        .unwrap_or(false)
}

/// Java-Argumente für beide Startarten
pub fn java_args(memory_mb: u32, extra: &str) -> String {
    let mut args = format!(
        "-Xmx{memory_mb}M -Xms512M -XX:+UseG1GC -XX:+UnlockExperimentalVMOptions -XX:G1NewSizePercent=20 -XX:G1ReservePercent=20 -XX:MaxGCPauseMillis=50 -XX:G1HeapRegionSize=32M"
    );
    if !extra.trim().is_empty() {
        args.push(' ');
        args.push_str(extra.trim());
    }
    args
}

fn write_profile(id: &str, name: &str, version_id: &str, memory_mb: u32, extra: &str) -> Result<()> {
    let file = paths::game().join("launcher_profiles.json");
    let mut root: Value = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| json!({}));
    if !root.is_object() {
        root = json!({});
    }
    let obj = root.as_object_mut().unwrap();
    obj.entry("settings").or_insert_with(|| json!({}));
    obj.entry("version").or_insert(json!(3));
    let profiles = obj.entry("profiles").or_insert_with(|| json!({}));
    if !profiles.is_object() {
        *profiles = json!({});
    }
    let profiles: &mut Map<String, Value> = profiles.as_object_mut().unwrap();

    // Der Ordner gehört nur dem NNN Launcher: Profile der Forge/NeoForge-Installer und gelöschter
    // Instanzen entfernen, sonst stehen sie im offiziellen Launcher vor unserem Profil
    let keys: Vec<String> = profiles.keys().cloned().collect();
    for k in keys {
        let custom = profiles[&k]["type"] == "custom";
        let remove = match k.strip_prefix("nnn-") {
            Some(inst) => !instances::dir(inst).exists(),
            None => custom,
        };
        if remove {
            profiles.remove(&k);
        }
    }

    let key = format!("nnn-{id}");
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    let created = profiles.get(&key).and_then(|p| p["created"].as_str()).unwrap_or(&now).to_string();
    let args = java_args(memory_mb, extra);
    profiles.insert(
        key,
        json!({
            "name": name,
            "type": "custom",
            "created": created,
            "lastUsed": now,
            "lastVersionId": version_id,
            "gameDir": instances::game_dir(id),
            "javaArgs": args,
            "icon": "Crafting_Table",
        }),
    );
    let tmp = file.with_extension("json.tmp");
    std::fs::create_dir_all(paths::game())?;
    std::fs::write(&tmp, serde_json::to_vec_pretty(&root)?)?;
    std::fs::rename(tmp, file)?;
    Ok(())
}
