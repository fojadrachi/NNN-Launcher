//! Instanzen (je ein Spielordner) und Pack-Sync. Dateiformat kompatibel zur C#-Version,
//! erweitert um Texturepacks und Shader.

use crate::{net, paths};
use anyhow::{anyhow, bail, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

/// Art eines Inhalts: bestimmt Ordner, Dateiendung und Liste im Manifest
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Mod,
    ResourcePack,
    Shader,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Mod, Kind::ResourcePack, Kind::Shader];

    pub fn from_index(i: i32) -> Kind {
        match i {
            1 => Kind::ResourcePack,
            2 => Kind::Shader,
            _ => Kind::Mod,
        }
    }
    pub fn from_str(s: &str) -> Kind {
        match s {
            "resourcepack" => Kind::ResourcePack,
            "shader" => Kind::Shader,
            _ => Kind::Mod,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Kind::Mod => "mod",
            Kind::ResourcePack => "resourcepack",
            Kind::Shader => "shader",
        }
    }
    pub fn folder(self) -> &'static str {
        match self {
            Kind::Mod => "mods",
            Kind::ResourcePack => "resourcepacks",
            Kind::Shader => "shaderpacks",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Kind::Mod => "Mod",
            Kind::ResourcePack => "Texturepack",
            Kind::Shader => "Shader",
        }
    }
    fn ext_ok(self, file: &str) -> bool {
        let f = file.to_lowercase();
        match self {
            Kind::Mod => f.ends_with(".jar"),
            _ => f.ends_with(".zip"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ModEntry {
    pub source: String,
    pub project_id: String,
    pub version_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_name: Option<String>,
    pub file_name: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    pub dependency: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct OverrideEntry {
    pub path: String,
    pub data: String,
    pub sha1: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct PackManifest {
    pub format_version: u32,
    pub name: String,
    pub minecraft: String,
    pub loader: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loader_version: Option<String>,
    pub mods: Vec<ModEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub resourcepacks: Vec<ModEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shaders: Vec<ModEntry>,
    pub overrides: Vec<OverrideEntry>,
}

impl Default for PackManifest {
    fn default() -> Self {
        Self {
            format_version: 1,
            name: String::new(),
            minecraft: String::new(),
            loader: "vanilla".into(),
            loader_version: None,
            mods: vec![],
            resourcepacks: vec![],
            shaders: vec![],
            overrides: vec![],
        }
    }
}

impl PackManifest {
    pub fn list(&self, k: Kind) -> &Vec<ModEntry> {
        match k {
            Kind::Mod => &self.mods,
            Kind::ResourcePack => &self.resourcepacks,
            Kind::Shader => &self.shaders,
        }
    }
    pub fn list_mut(&mut self, k: Kind) -> &mut Vec<ModEntry> {
        match k {
            Kind::Mod => &mut self.mods,
            Kind::ResourcePack => &mut self.resourcepacks,
            Kind::Shader => &mut self.shaders,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Instance {
    pub id: String,
    pub name: String,
    pub manifest: PackManifest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pack_id: Option<String>,
    pub pack_version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pack_role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_hash: Option<String>,
    pub managed_files: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u32>,
    pub created_at: i64,
    pub last_played: i64,
}

impl Instance {
    pub fn game_dir(&self) -> PathBuf { game_dir(&self.id) }
    /// Lokale Änderungen, die noch nicht im geteilten Pack sind
    pub fn dirty(&self) -> bool { self.pack_id.is_some() && self.published_hash.as_deref() != Some(&manifest_hash(&self.manifest)) }
    pub fn can_edit(&self) -> bool { self.pack_id.is_none() || matches!(self.pack_role.as_deref(), Some("owner" | "editor")) }
    /// Ist ein Shader-Loader (Iris, Oculus, OptiFine) installiert?
    pub fn has_shader_loader(&self) -> bool {
        let names = self.manifest.mods.iter().map(|m| format!("{} {}", m.name, m.file_name)).chain(local_files(self, Kind::Mod));
        names.map(|n| n.to_lowercase()).any(|n| n.contains("iris") || n.contains("oculus") || n.contains("optifine"))
    }
}

pub fn dir(id: &str) -> PathBuf { paths::instances().join(id) }
pub fn game_dir(id: &str) -> PathBuf { dir(id).join("game") }
fn file(id: &str) -> PathBuf { dir(id).join("instance.json") }

pub fn now() -> i64 { chrono::Utc::now().timestamp() }

fn check_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 40 || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        bail!("Ungültige Instanz-ID");
    }
    Ok(())
}

pub fn list() -> Vec<Instance> {
    let mut out: Vec<Instance> = std::fs::read_dir(paths::instances())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read(e.path().join("instance.json")).ok())
        .filter_map(|b| serde_json::from_slice::<Instance>(&b).ok())
        .collect();
    out.sort_by_key(|i| std::cmp::Reverse(i.last_played.max(i.created_at)));
    out
}

pub fn get(id: &str) -> Result<Instance> {
    check_id(id)?;
    let b = std::fs::read(file(id)).map_err(|_| anyhow!("Instanz nicht gefunden"))?;
    Ok(serde_json::from_slice(&b)?)
}

pub fn save(i: &Instance) -> Result<()> {
    let path = file(&i.id);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(i)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

pub fn create(name: &str, mc: &str, loader: &str, loader_version: Option<String>) -> Result<Instance> {
    let name = name.trim();
    if name.is_empty() {
        bail!("Name fehlt");
    }
    let mut slug: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    slug = slug.trim_matches('-').to_string();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    if slug.is_empty() {
        slug = "instanz".into();
    }
    slug.truncate(30);
    let mut id = slug.clone();
    let mut n = 2;
    while dir(&id).exists() {
        id = format!("{slug}-{n}");
        n += 1;
    }
    let inst = Instance {
        id: id.clone(),
        name: name.to_string(),
        manifest: PackManifest {
            name: name.to_string(),
            minecraft: mc.to_string(),
            loader: loader.to_string(),
            loader_version: if loader == "vanilla" { None } else { loader_version },
            ..Default::default()
        },
        created_at: now(),
        ..Default::default()
    };
    for k in Kind::ALL {
        std::fs::create_dir_all(game_dir(&id).join(k.folder()))?;
    }
    save(&inst)?;
    Ok(inst)
}

pub fn delete(id: &str) -> Result<()> {
    check_id(id)?;
    if dir(id).exists() {
        std::fs::remove_dir_all(dir(id))?;
    }
    Ok(())
}

fn same_project(a: &ModEntry, b: &ModEntry) -> bool {
    (a.source == b.source && !a.project_id.is_empty() && a.project_id == b.project_id) || a.file_name.eq_ignore_ascii_case(&b.file_name)
}

fn rel_path(k: Kind, file_name: &str) -> String { format!("{}/{}", k.folder(), file_name) }

fn item_path(id: &str, k: Kind, file_name: &str) -> Result<PathBuf> {
    if file_name.contains(['/', '\\', ':']) || !k.ext_ok(file_name) {
        bail!("Ungültiger Dateiname: {file_name}");
    }
    paths::safe_join(&game_dir(id), &rel_path(k, file_name))
}

pub async fn add_item(id: &str, k: Kind, m: ModEntry) -> Result<Instance> {
    if !m.url.starts_with("https://") {
        bail!("{}: Der Autor erlaubt keine Downloads über andere Launcher.", m.name);
    }
    let target = item_path(id, k, &m.file_name)?;
    net::download_all(vec![net::DlItem { url: m.url.clone(), path: target, sha1: m.sha1.clone(), size: m.size }], 1, |_, _| {}).await?;
    let mut i = get(id)?;
    let old: Vec<ModEntry> = i.manifest.list(k).iter().filter(|o| same_project(o, &m)).cloned().collect();
    for o in old {
        if !o.file_name.eq_ignore_ascii_case(&m.file_name) {
            let _ = std::fs::remove_file(item_path(id, k, &o.file_name)?);
        }
        i.manifest.list_mut(k).retain(|x| x != &o);
        let rel = rel_path(k, &o.file_name);
        i.managed_files.retain(|f| f != &rel);
    }
    i.managed_files.push(rel_path(k, &m.file_name));
    i.manifest.list_mut(k).push(m);
    save(&i)?;
    Ok(i)
}

/// Ausgeschaltete Dateien heißen `<name>.disabled` (wie bei Modrinth/Prism) – Minecraft lädt sie dann nicht.
pub const DISABLED: &str = ".disabled";

fn disabled_path(p: &std::path::Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(DISABLED);
    PathBuf::from(s)
}

/// Pfad der vorhandenen Datei (eingeschaltet oder als .disabled), falls sie da ist
pub fn existing_path(id: &str, k: Kind, file_name: &str) -> Option<PathBuf> {
    let p = item_path(id, k, file_name).ok()?;
    if p.exists() {
        return Some(p);
    }
    let d = disabled_path(&p);
    d.exists().then_some(d)
}

/// P2P-Einträge, deren Datei hier noch fehlt (wird nachgeladen, sobald ein Freund online ist)
pub fn pending_p2p(i: &Instance) -> Vec<(Kind, ModEntry)> {
    Kind::ALL
        .iter()
        .flat_map(|k| i.manifest.list(*k).iter().map(move |m| (*k, m)))
        .filter(|(k, m)| m.url.starts_with(crate::p2p::SCHEME) && existing_path(&i.id, *k, &m.file_name).is_none())
        .map(|(k, m)| (k, m.clone()))
        .collect()
}

/// Ist der Eintrag ausgeschaltet? (Datei liegt nur als .disabled vor)
pub fn is_disabled(id: &str, k: Kind, file_name: &str) -> bool {
    item_path(id, k, file_name).map(|p| !p.exists() && disabled_path(&p).exists()).unwrap_or(false)
}

/// Ein- oder ausschalten durch Umbenennen. Gilt nur lokal, das geteilte Pack bleibt unverändert.
pub fn set_enabled(id: &str, k: Kind, file_name: &str, enabled: bool) -> Result<()> {
    let on = item_path(id, k, file_name)?;
    let off = disabled_path(&on);
    let (from, to) = if enabled { (&off, &on) } else { (&on, &off) };
    if from.exists() {
        if to.exists() {
            std::fs::remove_file(to)?;
        }
        std::fs::rename(from, to).map_err(|e| anyhow!("{file_name} konnte nicht umbenannt werden – läuft Minecraft noch? ({e})"))?;
    }
    Ok(())
}

pub fn remove_item(id: &str, k: Kind, file_name: &str) -> Result<Instance> {
    let mut i = get(id)?;
    let path = item_path(id, k, file_name)?;
    let _ = std::fs::remove_file(disabled_path(&path));
    let _ = std::fs::remove_file(path);
    i.manifest.list_mut(k).retain(|m| !m.file_name.eq_ignore_ascii_case(file_name));
    let rel = rel_path(k, file_name);
    i.managed_files.retain(|f| !f.eq_ignore_ascii_case(&rel));
    save(&i)?;
    Ok(i)
}

/// Selbst hineinkopierte Dateien (nicht im Manifest), ausgeschaltete mit ihrem normalen Namen
pub fn local_files(i: &Instance, k: Kind) -> Vec<String> {
    let known: HashSet<String> = i.manifest.list(k).iter().map(|m| m.file_name.to_lowercase()).collect();
    let mut v: Vec<String> = std::fs::read_dir(i.game_dir().join(k.folder()))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .map(|n| match n.strip_suffix(DISABLED) {
            Some(base) => base.to_string(),
            None => n,
        })
        .filter(|n| k.ext_ok(n) && !known.contains(&n.to_lowercase()))
        .collect();
    v.sort();
    v.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    v
}

/// Gleicher Hash wie die C#-Version, damit "unveröffentlicht" nach dem Umstieg stimmt.
/// Texturepacks und Shader zählen nur mit, wenn welche da sind.
pub fn manifest_hash(m: &PackManifest) -> String {
    use sha2::{Digest, Sha256};
    let list = |v: &Vec<ModEntry>| {
        let mut l: Vec<String> = v.iter().map(|x| format!("{}:{}", x.file_name, x.sha1.clone().unwrap_or_default())).collect();
        l.sort();
        l.iter().map(|s| cs_json_str(s)).collect::<Vec<_>>().join(",")
    };
    // Exakt wie System.Text.Json (Standard-Encoder): feste Feldreihenfolge, HTML-kritische Zeichen,
    // '+' und Nicht-ASCII als \uXXXX
    let lv = m.loader_version.as_deref().map(cs_json_str).unwrap_or_else(|| "null".into());
    let mut s = format!(
        "{{\"Minecraft\":{},\"Loader\":{},\"LoaderVersion\":{},\"Mods\":[{}]",
        cs_json_str(&m.minecraft),
        cs_json_str(&m.loader),
        lv,
        list(&m.mods)
    );
    if !m.resourcepacks.is_empty() {
        s.push_str(&format!(",\"ResourcePacks\":[{}]", list(&m.resourcepacks)));
    }
    if !m.shaders.is_empty() {
        s.push_str(&format!(",\"Shaders\":[{}]", list(&m.shaders)));
    }
    s.push('}');
    hex::encode(Sha256::digest(s.as_bytes()))
}

fn cs_json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.encode_utf16() {
        match c {
            0x22 => out.push_str("\\u0022"),
            0x5C => out.push_str("\\\\"),
            0x08 => out.push_str("\\b"),
            0x0C => out.push_str("\\f"),
            0x0A => out.push_str("\\n"),
            0x0D => out.push_str("\\r"),
            0x09 => out.push_str("\\t"),
            0x26 | 0x27 | 0x2B | 0x3C | 0x3E | 0x60 => out.push_str(&format!("\\u{c:04X}")),
            0x20..=0x7E => out.push(c as u8 as char),
            _ => out.push_str(&format!("\\u{c:04X}")),
        }
    }
    out.push('"');
    out
}

const OVERRIDE_FOLDERS: [&str; 4] = ["config", "defaultconfigs", "kubejs", "scripts"];

pub fn collect_overrides(id: &str) -> Result<Vec<OverrideEntry>> {
    let game = game_dir(id);
    let mut out = Vec::new();
    let mut total = 0u64;
    for folder in OVERRIDE_FOLDERS {
        let mut stack = vec![game.join(folder)];
        let mut files = Vec::new();
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() { stack.push(p) } else { files.push(p) }
            }
        }
        files.sort();
        for f in files {
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            if len > 256_000 || matches!(ext.as_str(), "bak" | "log" | "tmp" | "lock") {
                continue;
            }
            total += len;
            if total > 20_000_000 {
                bail!("Config-Ordner zu groß zum Mitschicken (max. 20 MB).");
            }
            let bytes = std::fs::read(&f)?;
            let rel = f.strip_prefix(&game)?.to_string_lossy().replace('\\', "/");
            out.push(OverrideEntry { path: rel, sha1: net::sha1_bytes(&bytes), data: base64::engine::general_purpose::STANDARD.encode(&bytes) });
        }
    }
    Ok(out)
}

pub fn build_manifest(i: &Instance, include_configs: bool) -> Result<PackManifest> {
    Ok(PackManifest {
        name: i.name.clone(),
        overrides: if include_configs { collect_overrides(&i.id)? } else { vec![] },
        ..i.manifest.clone()
    })
}

/// Wendet eine Pack-Version an: neue/geänderte Dateien laden, nicht mehr enthaltene verwaltete
/// Dateien löschen. Eigene Dateien des Nutzers bleiben unangetastet.
pub async fn apply_manifest<F>(id: &str, manifest: PackManifest, version: i64, role: Option<String>, progress: F) -> Result<Instance>
where
    F: Fn(usize, usize) + Send + Sync + 'static,
{
    let mut i = get(id)?;
    let game = game_dir(id);
    let mut wanted: HashSet<String> = HashSet::new();
    let mut managed = Vec::new();
    let mut items = Vec::new();
    for k in Kind::ALL {
        for m in manifest.list(k) {
            if m.url.is_empty() {
                bail!("{}: keine Download-URL", m.name);
            }
            let rel = rel_path(k, &m.file_name);
            // Lokal ausgeschaltete Einträge bleiben aus (Datei wird ggf. als .disabled aktualisiert)
            let mut path = item_path(id, k, &m.file_name)?;
            if is_disabled(id, k, &m.file_name) {
                path = disabled_path(&path);
            }
            items.push(net::DlItem { url: m.url.clone(), path, sha1: m.sha1.clone(), size: m.size });
            wanted.insert(rel.to_lowercase());
            managed.push(rel);
        }
    }
    for o in &manifest.overrides {
        wanted.insert(o.path.to_lowercase());
        managed.push(o.path.clone());
    }
    // Erst alles laden, dann löschen – so bleibt die Instanz bei Fehlern spielbar.
    // P2P-Dateien sind "best effort": Ist gerade niemand online, der sie hat, kommen sie später.
    let (p2p_items, items): (Vec<_>, Vec<_>) = items.into_iter().partition(|i: &net::DlItem| i.url.starts_with(crate::p2p::SCHEME));
    net::download_all(items, 8, progress).await?;
    for item in p2p_items {
        if !item.path.exists() {
            let _ = net::download_one(&item).await;
        }
    }
    for o in &manifest.overrides {
        let p = paths::safe_join(&game, &o.path)?;
        if p.exists() && net::sha1_file(&p).ok().as_deref() == Some(o.sha1.as_str()) {
            continue;
        }
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, base64::engine::general_purpose::STANDARD.decode(&o.data)?)?;
    }
    for old in &i.managed_files {
        if !wanted.contains(&old.to_lowercase()) {
            if let Ok(p) = paths::safe_join(&game, old) {
                let _ = std::fs::remove_file(disabled_path(&p));
                let _ = std::fs::remove_file(p);
            }
        }
    }
    i.manifest = PackManifest { name: i.name.clone(), overrides: vec![], ..manifest };
    i.managed_files = managed;
    i.pack_version = version;
    if role.is_some() {
        i.pack_role = role;
    }
    i.published_hash = Some(manifest_hash(&i.manifest));
    save(&i)?;
    Ok(i)
}

pub fn link_pack(id: &str, pack_id: &str, version: i64, role: &str) -> Result<Instance> {
    let mut i = get(id)?;
    i.pack_id = Some(pack_id.to_string());
    i.pack_version = version;
    i.pack_role = Some(role.to_string());
    i.published_hash = Some(manifest_hash(&i.manifest));
    save(&i)?;
    Ok(i)
}

pub fn unlink_pack(id: &str) -> Result<Instance> {
    let mut i = get(id)?;
    i.pack_id = None;
    i.pack_version = 0;
    i.pack_role = None;
    i.published_hash = None;
    save(&i)?;
    Ok(i)
}

/// Übernimmt per Ordner-Scan erkannte Dateien ins Manifest und entfernt Einträge, deren Datei
/// von Hand gelöscht wurde. Die Dateien liegen schon im Ordner, es wird nichts geladen.
pub fn adopt(id: &str, found: Vec<(Kind, ModEntry)>, missing: &[(Kind, String)]) -> Result<Instance> {
    let mut i = get(id)?;
    for (k, file) in missing {
        i.manifest.list_mut(*k).retain(|m| !m.file_name.eq_ignore_ascii_case(file));
        let rel = rel_path(*k, file);
        i.managed_files.retain(|f| !f.eq_ignore_ascii_case(&rel));
    }
    for (k, m) in found {
        if !k.ext_ok(&m.file_name) || i.manifest.list(k).iter().any(|x| x.file_name.eq_ignore_ascii_case(&m.file_name)) {
            continue;
        }
        i.managed_files.push(rel_path(k, &m.file_name));
        i.manifest.list_mut(k).push(m);
    }
    save(&i)?;
    Ok(i)
}
