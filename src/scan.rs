//! Scannt die Ordner einer Instanz (mods, resourcepacks, shaderpacks) und erkennt Dateien über
//! ihre Prüfsumme bei Modrinth (SHA-1) und CurseForge (Fingerprint). Unbekannte Mods werden über
//! ihre eigenen Metadaten (fabric.mod.json, mods.toml, …) beschrieben.

use crate::instances::{Instance, Kind, ModEntry};
use crate::{api, net};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Eine Datei im Ordner, die nicht (oder anders) im Manifest steht
#[derive(Clone, Debug, Default)]
pub struct Found {
    pub kind: Option<Kind>,
    pub file_name: String,
    /// Bei Modrinth/CurseForge erkannt – mit Download-Link, kann ins Pack übernommen werden
    pub entry: Option<ModEntry>,
    /// Aus der Datei selbst gelesen
    pub meta_name: String,
    pub meta_version: String,
    pub icon_png: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Default)]
pub struct ScanResult {
    pub found: Vec<Found>,
    /// Einträge im Manifest, deren Datei fehlt (von Hand gelöscht)
    pub missing: Vec<(Kind, String)>,
}

// Prüfsummen merken, damit unveränderte Dateien nicht jedes Mal neu gelesen werden
#[derive(Clone)]
struct Hashes {
    sha1: String,
    murmur: u32,
}
static CACHE: Mutex<Option<HashMap<(PathBuf, u64, u128), Hashes>>> = Mutex::new(None);
// Ergebnisse der Online-Erkennung pro SHA-1 (None = nirgends gefunden)
static LOOKUP: Mutex<Option<HashMap<String, Option<ModEntry>>>> = Mutex::new(None);

fn hashes(path: &Path) -> Result<Hashes> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    let key = (path.to_path_buf(), meta.len(), mtime);
    if let Some(h) = CACHE.lock().unwrap().as_ref().and_then(|c| c.get(&key)) {
        return Ok(h.clone());
    }
    let data = std::fs::read(path)?;
    // CurseForge-Fingerprint nur, wenn CurseForge eingeschaltet ist (spart Zeit bei großen Packs)
    let h = Hashes { sha1: net::sha1_bytes(&data), murmur: if crate::mods::CURSEFORGE { cf_fingerprint(&data) } else { 0 } };
    CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, h.clone());
    Ok(h)
}

/// CurseForge-Fingerprint: MurmurHash2 (Seed 1) über die Datei ohne Leerzeichen, Tabs und Zeilenumbrüche
pub fn cf_fingerprint(data: &[u8]) -> u32 {
    let bytes: Vec<u8> = data.iter().copied().filter(|b| !matches!(b, 9 | 10 | 13 | 32)).collect();
    const M: u32 = 0x5bd1_e995;
    let len = bytes.len() as u32;
    let mut h: u32 = 1 ^ len;
    let mut chunks = bytes.chunks_exact(4);
    for c in &mut chunks {
        let mut k = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        k = k.wrapping_mul(M);
        k ^= k >> 24;
        k = k.wrapping_mul(M);
        h = h.wrapping_mul(M) ^ k;
    }
    let rest = chunks.remainder();
    if !rest.is_empty() {
        for (i, b) in rest.iter().enumerate() {
            h ^= (*b as u32) << (8 * i);
        }
        h = h.wrapping_mul(M);
    }
    h ^= h >> 13;
    h = h.wrapping_mul(M);
    h ^= h >> 15;
    h
}

fn ext_ok(k: Kind, name: &str) -> bool {
    let n = name.to_lowercase();
    if k == Kind::Mod { n.ends_with(".jar") } else { n.ends_with(".zip") }
}

pub async fn scan(inst: &Instance) -> Result<ScanResult> {
    let mut result = ScanResult::default();
    // Dateien sammeln, die nicht im Manifest stehen
    let mut todo: Vec<(Kind, String, PathBuf)> = Vec::new();
    for k in Kind::ALL {
        let dir = inst.game_dir().join(k.folder());
        let known: HashMap<String, &ModEntry> = inst.manifest.list(k).iter().map(|m| (m.file_name.to_lowercase(), m)).collect();
        let mut present = std::collections::HashSet::new();
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let Ok(name) = e.file_name().into_string() else { continue };
            if !ext_ok(k, &name) || !e.path().is_file() {
                continue;
            }
            present.insert(name.to_lowercase());
            if !known.contains_key(&name.to_lowercase()) {
                todo.push((k, name, e.path()));
            }
        }
        for m in inst.manifest.list(k) {
            if !present.contains(&m.file_name.to_lowercase()) {
                result.missing.push((k, m.file_name.clone()));
            }
        }
    }
    if todo.is_empty() {
        return Ok(result);
    }

    // Prüfsummen (im Hintergrund-Thread, Jars können groß sein)
    let paths: Vec<PathBuf> = todo.iter().map(|t| t.2.clone()).collect();
    let hs: Vec<Option<Hashes>> = tokio::task::spawn_blocking(move || paths.iter().map(|p| hashes(p).ok()).collect()).await?;

    // Online erkennen: erst Modrinth, dann CurseForge für den Rest
    let unknown: Vec<(String, u32)> = hs
        .iter()
        .flatten()
        .filter(|h| !LOOKUP.lock().unwrap().as_ref().is_some_and(|l| l.contains_key(&h.sha1)))
        .map(|h| (h.sha1.clone(), h.murmur))
        .collect();
    if !unknown.is_empty() {
        let mut found = modrinth_lookup(&unknown.iter().map(|u| u.0.clone()).collect::<Vec<_>>()).await.unwrap_or_default();
        let rest: Vec<(String, u32)> = unknown.iter().filter(|u| !found.contains_key(&u.0)).cloned().collect();
        if crate::mods::CURSEFORGE && !rest.is_empty() && api::logged_in() {
            if let Ok(cf) = curseforge_lookup(&rest).await {
                found.extend(cf);
            }
        }
        let mut l = LOOKUP.lock().unwrap();
        let l = l.get_or_insert_with(HashMap::new);
        for (sha, _) in &unknown {
            l.insert(sha.clone(), found.get(sha).cloned());
        }
    }

    for ((k, name, path), h) in todo.into_iter().zip(hs) {
        let entry = h.as_ref().and_then(|h| LOOKUP.lock().unwrap().as_ref().and_then(|l| l.get(&h.sha1).cloned()).flatten()).map(|mut e| {
            // Die Datei liegt mit ihrem eigenen Namen im Ordner – den behalten
            e.file_name = name.clone();
            e.sha1 = h.as_ref().map(|h| h.sha1.clone());
            e
        });
        let (meta_name, meta_version, icon_png) = if entry.is_none() || entry.as_ref().is_some_and(|e| e.icon_url.is_none()) {
            let p = path.clone();
            tokio::task::spawn_blocking(move || read_meta(&p)).await.unwrap_or_default()
        } else {
            Default::default()
        };
        result.found.push(Found { kind: Some(k), file_name: name, entry, meta_name, meta_version, icon_png });
    }
    Ok(result)
}

async fn modrinth_lookup(sha1s: &[String]) -> Result<HashMap<String, ModEntry>> {
    let res = net::client()
        .post("https://api.modrinth.com/v2/version_files")
        .json(&json!({ "hashes": sha1s, "algorithm": "sha1" }))
        .send()
        .await?
        .error_for_status()?;
    let versions: HashMap<String, Value> = res.json().await?;
    if versions.is_empty() {
        return Ok(HashMap::new());
    }
    // Projektnamen und Icons in einem Rutsch
    let ids: Vec<String> = versions.values().filter_map(|v| v["project_id"].as_str().map(String::from)).collect();
    let url = format!("https://api.modrinth.com/v2/projects?ids={}", urlencoding::encode(&serde_json::to_string(&ids)?));
    let projects: Vec<Value> = net::get_json(&url).await.ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let by_id: HashMap<String, &Value> = projects.iter().filter_map(|p| Some((p["id"].as_str()?.to_string(), p))).collect();

    let mut out = HashMap::new();
    for (sha, v) in &versions {
        let files = v["files"].as_array().cloned().unwrap_or_default();
        let Some(f) = files.iter().find(|f| f["hashes"]["sha1"].as_str() == Some(sha.as_str())).or_else(|| files.first()) else { continue };
        let pid = v["project_id"].as_str().unwrap_or_default();
        let p = by_id.get(pid);
        out.insert(
            sha.clone(),
            ModEntry {
                source: "modrinth".into(),
                project_id: pid.into(),
                version_id: v["id"].as_str().unwrap_or_default().into(),
                name: p.and_then(|p| p["title"].as_str()).unwrap_or(v["name"].as_str().unwrap_or(pid)).into(),
                version_name: v["version_number"].as_str().map(String::from),
                file_name: f["filename"].as_str().unwrap_or_default().into(),
                url: f["url"].as_str().unwrap_or_default().into(),
                sha1: Some(sha.clone()),
                size: f["size"].as_u64().unwrap_or(0),
                icon_url: p.and_then(|p| p["icon_url"].as_str()).filter(|s| !s.is_empty()).map(String::from),
                ..Default::default()
            },
        );
    }
    Ok(out)
}

async fn curseforge_lookup(items: &[(String, u32)]) -> Result<HashMap<String, ModEntry>> {
    let fps: Vec<u32> = items.iter().map(|i| i.1).collect();
    let d = api::curseforge_post("/v1/fingerprints/432", json!({ "fingerprints": fps })).await?;
    let matches = d["data"]["exactMatches"].as_array().cloned().unwrap_or_default();
    if matches.is_empty() {
        return Ok(HashMap::new());
    }
    let mod_ids: Vec<i64> = matches.iter().filter_map(|m| m["id"].as_i64()).collect();
    let mods = api::curseforge_post("/v1/mods", json!({ "modIds": mod_ids })).await.unwrap_or(Value::Null);
    let by_id: HashMap<i64, &Value> = mods["data"].as_array().into_iter().flatten().filter_map(|m| Some((m["id"].as_i64()?, m))).collect();
    let by_fp: HashMap<u32, &String> = items.iter().map(|i| (i.1, &i.0)).collect();

    let mut out = HashMap::new();
    for m in &matches {
        let f = &m["file"];
        let Some(fp) = f["fileFingerprint"].as_u64().map(|x| x as u32) else { continue };
        let Some(sha) = by_fp.get(&fp) else { continue };
        let Some(url) = f["downloadUrl"].as_str().filter(|s| !s.is_empty()) else { continue };
        let mid = m["id"].as_i64().unwrap_or(0);
        let info = by_id.get(&mid);
        out.insert(
            (*sha).clone(),
            ModEntry {
                source: "curseforge".into(),
                project_id: mid.to_string(),
                version_id: f["id"].as_i64().unwrap_or(0).to_string(),
                name: info.and_then(|i| i["name"].as_str()).unwrap_or(f["displayName"].as_str().unwrap_or_default()).into(),
                version_name: f["displayName"].as_str().map(String::from),
                file_name: f["fileName"].as_str().unwrap_or_default().into(),
                url: url.into(),
                sha1: Some((*sha).clone()),
                size: f["fileLength"].as_u64().unwrap_or(0),
                icon_url: info.and_then(|i| i["logo"]["thumbnailUrl"].as_str()).map(String::from),
                ..Default::default()
            },
        );
    }
    Ok(out)
}

/// Name, Version und Icon aus der Datei selbst (Fabric, Quilt, Forge, NeoForge, Packs)
fn read_meta(path: &Path) -> (String, String, Option<Vec<u8>>) {
    let Ok(file) = std::fs::File::open(path) else { return Default::default() };
    let Ok(mut zip) = zip::ZipArchive::new(file) else { return Default::default() };
    let mut text = |name: &str| -> Option<String> {
        let mut e = zip.by_name(name).ok()?;
        let mut s = String::new();
        e.read_to_string(&mut s).ok()?;
        Some(s)
    };
    let mut name = String::new();
    let mut version = String::new();
    let mut icon_path: Option<String> = None;

    if let Some(j) = text("fabric.mod.json").and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
        name = j["name"].as_str().or(j["id"].as_str()).unwrap_or_default().into();
        version = j["version"].as_str().unwrap_or_default().into();
        icon_path = j["icon"].as_str().map(String::from).or_else(|| j["icon"].as_object().and_then(|o| o.values().last()).and_then(|v| v.as_str()).map(String::from));
    } else if let Some(j) = text("quilt.mod.json").and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
        let l = &j["quilt_loader"];
        name = l["metadata"]["name"].as_str().or(l["id"].as_str()).unwrap_or_default().into();
        version = l["version"].as_str().unwrap_or_default().into();
        icon_path = l["metadata"]["icon"].as_str().map(String::from);
    } else if let Some(t) = text("META-INF/neoforge.mods.toml").or_else(|| text("META-INF/mods.toml")) {
        let get = |key: &str| regex::Regex::new(&format!(r#"(?m)^\s*{key}\s*=\s*"([^"]*)""#)).ok().and_then(|re| re.captures(&t).map(|c| c[1].to_string()));
        name = get("displayName").unwrap_or_default();
        version = get("version").unwrap_or_default();
        icon_path = get("logoFile");
        if version.contains("${") {
            // ${file.jarVersion} steht im Manifest der Jar
            version = text("META-INF/MANIFEST.MF")
                .and_then(|m| m.lines().find_map(|l| l.strip_prefix("Implementation-Version:").map(|v| v.trim().to_string())))
                .unwrap_or_default();
        }
    } else if let Some(j) = text("pack.mcmeta").and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
        // Texturepack: Beschreibung als Untertitel, pack.png als Icon
        version = match &j["pack"]["description"] {
            Value::String(s) => s.clone(),
            v => v["text"].as_str().unwrap_or_default().to_string(),
        };
        icon_path = Some("pack.png".into());
    }
    if icon_path.is_none() && zip.by_name("pack.png").is_ok() {
        icon_path = Some("pack.png".into());
    }
    let icon = icon_path.and_then(|p| {
        let mut e = zip.by_name(p.trim_start_matches('/')).ok()?;
        let mut b = Vec::new();
        e.read_to_end(&mut b).ok()?;
        (b.len() < 2_000_000).then_some(b)
    });
    // Formatcodes (§a usw.) aus Texten entfernen
    let clean = |s: String| {
        let mut out = String::new();
        let mut skip = false;
        for c in s.chars() {
            if skip {
                skip = false;
            } else if c == '§' {
                skip = true;
            } else {
                out.push(c);
            }
        }
        out.trim().to_string()
    };
    (clean(name), clean(version), icon)
}
