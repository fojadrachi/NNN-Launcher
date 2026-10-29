//! Suche und Auflösung bei Modrinth und CurseForge: Mods (mit Pflicht-Abhängigkeiten),
//! Texturepacks und Shader.

use crate::instances::{Instance, Kind, ModEntry};
use crate::{api, loaders, net};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::HashSet;

const MR: &str = "https://api.modrinth.com/v2";

/// CurseForge ist vorerst ausgeschaltet (Suche, Auswahl in der Oberfläche, Ordner-Scan).
/// Zum Einschalten auf `true` setzen – der Server-Proxy ist schon vorbereitet.
pub const CURSEFORGE: bool = false;

#[derive(Clone, Debug, Default)]
pub struct Hit {
    pub source: String,
    pub id: String,
    pub title: String,
    pub description: String,
    pub icon: Option<String>,
    pub downloads: u64,
    pub author: String,
    pub url: String,
}

fn mr_loaders(kind: Kind, loader: &str, mc: &str) -> Vec<&'static str> {
    match kind {
        Kind::ResourcePack => vec!["minecraft"],
        Kind::Shader => vec!["iris", "optifine", "canvas"],
        Kind::Mod => match loader {
            "fabric" => vec!["fabric"],
            "quilt" => vec!["quilt", "fabric"],
            "forge" => vec!["forge"],
            "neoforge" if mc == "1.20.1" => vec!["neoforge", "forge"],
            "neoforge" => vec!["neoforge"],
            _ => vec![],
        },
    }
}

fn mr_type(kind: Kind) -> &'static str {
    match kind { Kind::Mod => "mod", Kind::ResourcePack => "resourcepack", Kind::Shader => "shader" }
}

fn cf_class(kind: Kind) -> u32 {
    match kind { Kind::Mod => 6, Kind::ResourcePack => 12, Kind::Shader => 6552 }
}

fn cf_loader(loader: &str) -> Option<u8> {
    match loader { "forge" => Some(1), "fabric" => Some(4), "quilt" => Some(5), "neoforge" => Some(6), _ => None }
}

pub async fn search(kind: Kind, source: &str, query: &str, inst: Option<&Instance>, offset: usize) -> Result<(Vec<Hit>, usize)> {
    if CURSEFORGE && source == "curseforge" { cf_search(kind, query, inst, offset).await } else { mr_search(kind, query, inst, offset).await }
}

async fn mr_search(kind: Kind, query: &str, inst: Option<&Instance>, offset: usize) -> Result<(Vec<Hit>, usize)> {
    let mut facets = vec![vec![format!("project_type:{}", mr_type(kind))]];
    if let Some(i) = inst {
        if kind == Kind::Mod {
            let l = mr_loaders(kind, &i.manifest.loader, &i.manifest.minecraft);
            if !l.is_empty() {
                facets.push(l.iter().map(|x| format!("categories:{x}")).collect());
            }
        }
        facets.push(vec![format!("versions:{}", i.manifest.minecraft)]);
    }
    let url = format!(
        "{MR}/search?query={}&limit=20&offset={offset}&index={}&facets={}",
        urlencoding::encode(query),
        if query.is_empty() { "downloads" } else { "relevance" },
        urlencoding::encode(&serde_json::to_string(&facets)?)
    );
    let d = net::get_json(&url).await?;
    let path = match kind { Kind::Mod => "mod", Kind::ResourcePack => "resourcepack", Kind::Shader => "shader" };
    let hits = d["hits"].as_array().into_iter().flatten().map(|h| Hit {
        source: "modrinth".into(),
        id: h["project_id"].as_str().unwrap_or_default().into(),
        title: h["title"].as_str().unwrap_or_default().into(),
        description: h["description"].as_str().unwrap_or_default().into(),
        icon: h["icon_url"].as_str().filter(|s| !s.is_empty()).map(String::from),
        downloads: h["downloads"].as_u64().unwrap_or(0),
        author: h["author"].as_str().unwrap_or_default().into(),
        url: format!("https://modrinth.com/{path}/{}", h["slug"].as_str().unwrap_or_default()),
    });
    Ok((hits.collect(), d["total_hits"].as_u64().unwrap_or(0) as usize))
}

async fn cf_search(kind: Kind, query: &str, inst: Option<&Instance>, offset: usize) -> Result<(Vec<Hit>, usize)> {
    let mut q = format!(
        "/v1/mods/search?gameId=432&classId={}&searchFilter={}&sortField={}&sortOrder=desc&pageSize=20&index={offset}",
        cf_class(kind),
        urlencoding::encode(query),
        if query.is_empty() { 2 } else { 1 }
    );
    if let Some(i) = inst {
        q.push_str(&format!("&gameVersion={}", i.manifest.minecraft));
        if kind == Kind::Mod {
            if let Some(t) = cf_loader(&i.manifest.loader) {
                q.push_str(&format!("&modLoaderType={t}"));
            }
        }
    }
    let d = api::curseforge(&q).await?;
    let hits = d["data"].as_array().into_iter().flatten().map(|m| Hit {
        source: "curseforge".into(),
        id: m["id"].as_i64().unwrap_or(0).to_string(),
        title: m["name"].as_str().unwrap_or_default().into(),
        description: m["summary"].as_str().unwrap_or_default().into(),
        icon: m["logo"]["thumbnailUrl"].as_str().filter(|s| !s.is_empty()).map(String::from),
        downloads: m["downloadCount"].as_f64().unwrap_or(0.0) as u64,
        author: m["authors"][0]["name"].as_str().unwrap_or_default().into(),
        url: m["links"]["websiteUrl"].as_str().unwrap_or_default().into(),
    });
    let total = d["pagination"]["totalCount"].as_u64().unwrap_or(0).min(10_000) as usize;
    Ok((hits.collect(), total))
}

/// Inhalt auflösen; bei Mods inklusive Pflicht-Abhängigkeiten (ohne bereits installierte)
pub async fn resolve_with_deps(kind: Kind, source: &str, id: &str, title: &str, icon: Option<String>, inst: &Instance) -> Result<Vec<ModEntry>> {
    let have: HashSet<String> = inst.manifest.mods.iter().map(|m| format!("{}:{}", m.source, m.project_id)).collect();
    let (main, mut queue) = resolve(kind, source, id, Some(title.to_string()), icon, inst).await?;
    let mut seen: HashSet<String> = HashSet::from([format!("{}:{}", main.source, main.project_id)]);
    let mut out = vec![main];
    if kind != Kind::Mod {
        return Ok(out);
    }
    while let Some(dep) = queue.pop() {
        let key = format!("{source}:{dep}");
        if seen.contains(&key) || have.contains(&key) {
            continue;
        }
        seen.insert(key);
        // Abhängigkeiten, die es für diese Version nicht gibt, überspringen statt alles abzubrechen
        if let Ok((mut m, more)) = resolve(Kind::Mod, source, &dep, None, None, inst).await {
            m.dependency = true;
            out.push(m);
            queue.extend(more);
        }
    }
    Ok(out)
}

async fn resolve(kind: Kind, source: &str, id: &str, title: Option<String>, icon: Option<String>, inst: &Instance) -> Result<(ModEntry, Vec<String>)> {
    if source == "curseforge" { cf_resolve(kind, id, title, icon, inst).await } else { mr_resolve(kind, id, title, icon, inst).await }
}

async fn mr_versions(id: &str, mc: Option<&str>, loaders: &[&str]) -> Result<Vec<Value>> {
    let mut url = format!("{MR}/project/{id}/version?");
    if let Some(mc) = mc {
        url.push_str(&format!("game_versions={}&", urlencoding::encode(&format!("[\"{mc}\"]"))));
    }
    if !loaders.is_empty() {
        url.push_str(&format!("loaders={}", urlencoding::encode(&serde_json::to_string(loaders)?)));
    }
    Ok(net::get_json(&url).await?.as_array().cloned().unwrap_or_default())
}

async fn mr_resolve(kind: Kind, id: &str, title: Option<String>, icon: Option<String>, inst: &Instance) -> Result<(ModEntry, Vec<String>)> {
    let mc = &inst.manifest.minecraft;
    let loaders = mr_loaders(kind, &inst.manifest.loader, mc);
    let (title, icon) = match title {
        Some(t) => (t, icon),
        None => {
            let p = net::get_json(&format!("{MR}/project/{id}")).await?;
            if p["project_type"] != mr_type(kind) {
                anyhow::bail!("falscher Projekttyp");
            }
            (p["title"].as_str().unwrap_or(id).to_string(), p["icon_url"].as_str().map(String::from))
        }
    };
    let mut list = mr_versions(id, Some(mc), &loaders).await?;
    // Texturepacks und Shader laufen oft auch auf Versionen, die nicht ausdrücklich angegeben sind
    if list.is_empty() && kind != Kind::Mod {
        list = mr_versions(id, None, &loaders).await?;
    }
    let main_loader = Value::String(loaders.first().copied().unwrap_or_default().to_string());
    let pick = list
        .iter()
        .find(|v| v["version_type"] == "release" && v["loaders"].as_array().is_some_and(|l| l.contains(&main_loader)))
        .or_else(|| list.iter().find(|v| v["loaders"].as_array().is_some_and(|l| l.contains(&main_loader))))
        .or_else(|| list.first())
        .ok_or_else(|| anyhow!("{title}: keine Version für {mc}{}", if kind == Kind::Mod { format!(" / {}", loaders::display(&inst.manifest.loader)) } else { String::new() }))?;
    let files = pick["files"].as_array().cloned().unwrap_or_default();
    let f = files.iter().find(|f| f["primary"] == true).or_else(|| files.first()).ok_or_else(|| anyhow!("{title}: keine Datei"))?;
    let deps = pick["dependencies"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["dependency_type"] == "required")
        .filter_map(|d| d["project_id"].as_str().map(String::from))
        .collect();
    Ok((
        ModEntry {
            source: "modrinth".into(),
            project_id: pick["project_id"].as_str().unwrap_or(id).into(),
            version_id: pick["id"].as_str().unwrap_or_default().into(),
            name: title,
            version_name: pick["version_number"].as_str().map(String::from),
            file_name: f["filename"].as_str().unwrap_or_default().into(),
            url: f["url"].as_str().unwrap_or_default().into(),
            sha1: f["hashes"]["sha1"].as_str().map(String::from),
            size: f["size"].as_u64().unwrap_or(0),
            icon_url: icon,
            ..Default::default()
        },
        deps,
    ))
}

async fn cf_files(id: &str, mc: Option<&str>, loader: Option<u8>) -> Result<Vec<Value>> {
    let mut q = format!("/v1/mods/{id}/files?pageSize=30");
    if let Some(mc) = mc {
        q.push_str(&format!("&gameVersion={mc}"));
    }
    if let Some(t) = loader {
        q.push_str(&format!("&modLoaderType={t}"));
    }
    Ok(api::curseforge(&q).await?["data"].as_array().cloned().unwrap_or_default())
}

async fn cf_resolve(kind: Kind, id: &str, title: Option<String>, icon: Option<String>, inst: &Instance) -> Result<(ModEntry, Vec<String>)> {
    let mc = &inst.manifest.minecraft;
    let loader = if kind == Kind::Mod { cf_loader(&inst.manifest.loader) } else { None };
    let mut files = cf_files(id, Some(mc), loader).await?;
    if files.is_empty() && kind == Kind::Mod && inst.manifest.loader == "neoforge" && mc == "1.20.1" {
        files = cf_files(id, Some(mc), Some(1)).await?;
    }
    if files.is_empty() && kind == Kind::Mod && inst.manifest.loader == "quilt" {
        files = cf_files(id, Some(mc), Some(4)).await?;
    }
    if files.is_empty() && kind != Kind::Mod {
        files = cf_files(id, None, None).await?;
    }
    let (title, icon) = match title {
        Some(t) => (t, icon),
        None => {
            let m = api::curseforge(&format!("/v1/mods/{id}")).await?;
            (m["data"]["name"].as_str().unwrap_or(id).to_string(), m["data"]["logo"]["thumbnailUrl"].as_str().map(String::from))
        }
    };
    files.sort_by(|a, b| b["fileDate"].as_str().cmp(&a["fileDate"].as_str()));
    let f = files
        .iter()
        .find(|f| f["releaseType"] == 1)
        .or_else(|| files.first())
        .ok_or_else(|| anyhow!("{title}: keine Datei für {mc}"))?;
    let url = f["downloadUrl"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| anyhow!("{title}: Der Autor erlaubt keine Downloads über andere Launcher."))?;
    let sha1 = f["hashes"].as_array().into_iter().flatten().find(|h| h["algo"] == 1).and_then(|h| h["value"].as_str()).map(String::from);
    let deps = if kind == Kind::Mod {
        f["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| d["relationType"] == 3)
            .filter_map(|d| d["modId"].as_i64().map(|i| i.to_string()))
            .collect()
    } else {
        vec![]
    };
    Ok((
        ModEntry {
            source: "curseforge".into(),
            project_id: id.into(),
            version_id: f["id"].as_i64().unwrap_or(0).to_string(),
            name: title,
            version_name: f["displayName"].as_str().map(String::from),
            file_name: f["fileName"].as_str().unwrap_or_default().into(),
            url: url.into(),
            sha1,
            size: f["fileLength"].as_u64().unwrap_or(0),
            icon_url: icon,
            ..Default::default()
        },
        deps,
    ))
}
