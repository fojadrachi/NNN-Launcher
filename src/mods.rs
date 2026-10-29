//! Mod-Suche und -Auflösung bei Modrinth und CurseForge, inklusive Pflicht-Abhängigkeiten.

use crate::instances::{Instance, ModEntry};
use crate::{api, loaders, net};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::HashSet;

const MR: &str = "https://api.modrinth.com/v2";

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

fn mr_loaders(loader: &str, mc: &str) -> Vec<&'static str> {
    match loader {
        "fabric" => vec!["fabric"],
        "quilt" => vec!["quilt", "fabric"],
        "forge" => vec!["forge"],
        "neoforge" if mc == "1.20.1" => vec!["neoforge", "forge"],
        "neoforge" => vec!["neoforge"],
        _ => vec![],
    }
}

fn cf_loader(loader: &str) -> Option<u8> {
    match loader { "forge" => Some(1), "fabric" => Some(4), "quilt" => Some(5), "neoforge" => Some(6), _ => None }
}

pub async fn search(source: &str, query: &str, inst: Option<&Instance>, offset: usize) -> Result<(Vec<Hit>, usize)> {
    if source == "curseforge" { cf_search(query, inst, offset).await } else { mr_search(query, inst, offset).await }
}

async fn mr_search(query: &str, inst: Option<&Instance>, offset: usize) -> Result<(Vec<Hit>, usize)> {
    let mut facets = vec![vec!["project_type:mod".to_string()]];
    if let Some(i) = inst {
        let l = mr_loaders(&i.manifest.loader, &i.manifest.minecraft);
        if !l.is_empty() {
            facets.push(l.iter().map(|x| format!("categories:{x}")).collect());
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
    let hits = d["hits"].as_array().into_iter().flatten().map(|h| Hit {
        source: "modrinth".into(),
        id: h["project_id"].as_str().unwrap_or_default().into(),
        title: h["title"].as_str().unwrap_or_default().into(),
        description: h["description"].as_str().unwrap_or_default().into(),
        icon: h["icon_url"].as_str().filter(|s| !s.is_empty()).map(String::from),
        downloads: h["downloads"].as_u64().unwrap_or(0),
        author: h["author"].as_str().unwrap_or_default().into(),
        url: format!("https://modrinth.com/mod/{}", h["slug"].as_str().unwrap_or_default()),
    });
    Ok((hits.collect(), d["total_hits"].as_u64().unwrap_or(0) as usize))
}

async fn cf_search(query: &str, inst: Option<&Instance>, offset: usize) -> Result<(Vec<Hit>, usize)> {
    let mut q = format!(
        "/v1/mods/search?gameId=432&classId=6&searchFilter={}&sortField={}&sortOrder=desc&pageSize=20&index={offset}",
        urlencoding::encode(query),
        if query.is_empty() { 2 } else { 1 }
    );
    if let Some(i) = inst {
        q.push_str(&format!("&gameVersion={}", i.manifest.minecraft));
        if let Some(t) = cf_loader(&i.manifest.loader) {
            q.push_str(&format!("&modLoaderType={t}"));
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

/// Mod samt Pflicht-Abhängigkeiten auflösen (ohne bereits installierte)
pub async fn resolve_with_deps(source: &str, id: &str, title: &str, icon: Option<String>, inst: &Instance) -> Result<Vec<ModEntry>> {
    let have: HashSet<String> = inst.manifest.mods.iter().map(|m| format!("{}:{}", m.source, m.project_id)).collect();
    let (main, mut queue) = resolve(source, id, Some(title.to_string()), icon, inst).await?;
    let mut seen: HashSet<String> = HashSet::from([format!("{}:{}", main.source, main.project_id)]);
    let mut out = vec![main];
    while let Some(dep) = queue.pop() {
        let key = format!("{source}:{dep}");
        if seen.contains(&key) || have.contains(&key) {
            continue;
        }
        seen.insert(key);
        // Abhängigkeiten, die es für diese Version nicht gibt, überspringen statt alles abzubrechen
        if let Ok((mut m, more)) = resolve(source, &dep, None, None, inst).await {
            m.dependency = true;
            out.push(m);
            queue.extend(more);
        }
    }
    Ok(out)
}

async fn resolve(source: &str, id: &str, title: Option<String>, icon: Option<String>, inst: &Instance) -> Result<(ModEntry, Vec<String>)> {
    if source == "curseforge" { cf_resolve(id, title, icon, inst).await } else { mr_resolve(id, title, icon, inst).await }
}

async fn mr_resolve(id: &str, title: Option<String>, icon: Option<String>, inst: &Instance) -> Result<(ModEntry, Vec<String>)> {
    let mc = &inst.manifest.minecraft;
    let loaders = mr_loaders(&inst.manifest.loader, mc);
    let (title, icon) = match title {
        Some(t) => (t, icon),
        None => {
            let p = net::get_json(&format!("{MR}/project/{id}")).await?;
            if p["project_type"] != "mod" {
                anyhow::bail!("kein Mod");
            }
            (p["title"].as_str().unwrap_or(id).to_string(), p["icon_url"].as_str().map(String::from))
        }
    };
    let mut url = format!("{MR}/project/{id}/version?game_versions={}", urlencoding::encode(&format!("[\"{mc}\"]")));
    if !loaders.is_empty() {
        url.push_str(&format!("&loaders={}", urlencoding::encode(&serde_json::to_string(&loaders)?)));
    }
    let versions = net::get_json(&url).await?;
    let list = versions.as_array().cloned().unwrap_or_default();
    let main_loader = Value::String(loaders.first().copied().unwrap_or_default().to_string());
    let pick = list
        .iter()
        .find(|v| v["version_type"] == "release" && v["loaders"].as_array().is_some_and(|l| l.contains(&main_loader)))
        .or_else(|| list.iter().find(|v| v["loaders"].as_array().is_some_and(|l| l.contains(&main_loader))))
        .or_else(|| list.first())
        .ok_or_else(|| anyhow!("{title}: keine Version für {mc} / {}", loaders::display(&inst.manifest.loader)))?;
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

async fn cf_resolve(id: &str, title: Option<String>, icon: Option<String>, inst: &Instance) -> Result<(ModEntry, Vec<String>)> {
    let mc = &inst.manifest.minecraft;
    let files_for = |t: Option<u8>| {
        let mut q = format!("/v1/mods/{id}/files?gameVersion={mc}&pageSize=30");
        if let Some(t) = t {
            q.push_str(&format!("&modLoaderType={t}"));
        }
        async move { api::curseforge(&q).await.map(|d| d["data"].as_array().cloned().unwrap_or_default()) }
    };
    let mut files = files_for(cf_loader(&inst.manifest.loader)).await?;
    if files.is_empty() && inst.manifest.loader == "neoforge" && mc == "1.20.1" {
        files = files_for(Some(1)).await?;
    }
    if files.is_empty() && inst.manifest.loader == "quilt" {
        files = files_for(Some(4)).await?;
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
        .ok_or_else(|| anyhow!("{title}: keine Datei für {mc} / {}", loaders::display(&inst.manifest.loader)))?;
    let url = f["downloadUrl"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| anyhow!("{title}: Der Autor erlaubt keine Downloads über andere Launcher."))?;
    let sha1 = f["hashes"].as_array().into_iter().flatten().find(|h| h["algo"] == 1).and_then(|h| h["value"].as_str()).map(String::from);
    let deps = f["dependencies"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["relationType"] == 3)
        .filter_map(|d| d["modId"].as_i64().map(|i| i.to_string()))
        .collect();
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
