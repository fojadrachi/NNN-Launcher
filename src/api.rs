//! NNN-Server: Konten, Freunde, Packs, Versionen, Share-Links und der CurseForge-Proxy.
//! Die Adresse ist fest eingebaut – der Launcher spricht nur mit diesem Server.

use crate::instances::PackManifest;
use crate::net;
use crate::session::User;
use anyhow::{anyhow, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::RwLock;

pub const SERVER: &str = "https://launcher.nnn-server.de";

/// Release-Builds sprechen nur mit SERVER. Entwickler-Builds dürfen per NNN_SERVER auf einen Testserver zeigen.
fn server() -> String {
    #[cfg(debug_assertions)]
    if let Ok(s) = std::env::var("NNN_SERVER") {
        return s;
    }
    SERVER.to_string()
}

static TOKEN: RwLock<Option<String>> = RwLock::new(None);

pub fn set_token(t: Option<String>) { *TOKEN.write().unwrap() = t; }
pub fn logged_in() -> bool { TOKEN.read().unwrap().is_some() }

/// Fehler, bei dem die Sitzung abgelaufen ist
#[derive(Debug)]
pub struct Unauthorized;
impl std::fmt::Display for Unauthorized {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("Sitzung abgelaufen, bitte neu anmelden") }
}
impl std::error::Error for Unauthorized {}

async fn request(method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value> {
    let mut req = net::client().request(method, format!("{}{path}", server()));
    if let Some(t) = TOKEN.read().unwrap().clone() {
        req = req.bearer_auth(t);
    }
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.send().await.map_err(|_| anyhow!("NNN-Server nicht erreichbar"))?;
    let status = res.status();
    let data: Value = res.json().await.unwrap_or(Value::Null);
    if status == reqwest::StatusCode::UNAUTHORIZED && !path.starts_with("/auth") && logged_in() {
        return Err(Unauthorized.into());
    }
    if !status.is_success() {
        return Err(anyhow!(data["error"].as_str().map(String::from).unwrap_or_else(|| format!("Serverfehler ({})", status.as_u16()))));
    }
    Ok(data)
}

async fn get<T: DeserializeOwned>(path: &str) -> Result<T> { Ok(serde_json::from_value(request(reqwest::Method::GET, path, None).await?)?) }
async fn post(path: &str, body: Value) -> Result<Value> { request(reqwest::Method::POST, path, Some(body)).await }

// ---------- Konten ----------

#[derive(Deserialize)]
pub struct AuthResult { pub token: String, pub user: User }

pub async fn login(username: &str, password: &str) -> Result<AuthResult> {
    Ok(serde_json::from_value(post("/auth/login", json!({ "username": username, "password": password })).await?)?)
}
pub async fn register(username: &str, password: &str) -> Result<AuthResult> {
    Ok(serde_json::from_value(post("/auth/register", json!({ "username": username, "password": password })).await?)?)
}
pub async fn logout() { let _ = post("/auth/logout", json!({})).await; }

#[derive(Deserialize)]
struct Me { user: User }

/// Eigenes Konto inkl. Badges
pub async fn me() -> Result<User> { Ok(get::<Me>("/me").await?.user) }

pub fn token() -> Option<String> { TOKEN.read().unwrap().clone() }

// ---------- Badge-Codes ----------

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct BadgeCode { pub code: String, pub badge: String, pub max_uses: Option<i64>, pub uses: i64, pub created_at: i64, pub expires_at: Option<i64> }

#[derive(Deserialize)]
struct BadgeCodes { codes: Vec<BadgeCode> }

/// Code einlösen, gibt die neue Badge-Liste zurück
pub async fn redeem_badge(code: &str) -> Result<(String, Vec<String>)> {
    let v = post("/badges/redeem", json!({ "code": code })).await?;
    let badges = v["badges"].as_array().into_iter().flatten().filter_map(|b| b.as_str().map(String::from)).collect();
    Ok((v["badge"].as_str().unwrap_or_default().to_string(), badges))
}

/// Nur für den Owner
pub async fn badge_codes() -> Result<Vec<BadgeCode>> { Ok(get::<BadgeCodes>("/badges/codes").await?.codes) }
pub async fn create_badge_code(badge: &str, max_uses: i64, days: i64) -> Result<String> {
    let v = post("/badges/codes", json!({ "badge": badge, "maxUses": max_uses, "expiresInDays": days })).await?;
    Ok(v["code"].as_str().unwrap_or_default().to_string())
}
pub async fn delete_badge_code(code: &str) -> Result<()> {
    request(reqwest::Method::DELETE, &format!("/badges/codes/{code}"), None).await.map(|_| ())
}

// ---------- Freunde ----------

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Friend { pub id: i64, pub username: String, #[serde(default)] pub since: i64, #[serde(default)] pub badges: Vec<String> }

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Friends { pub friends: Vec<Friend>, pub incoming: Vec<Friend>, pub outgoing: Vec<Friend> }

pub async fn friends() -> Result<Friends> { get("/friends").await }
pub async fn friend_request(username: &str) -> Result<String> {
    Ok(post("/friends/request", json!({ "username": username })).await?["status"].as_str().unwrap_or("pending").to_string())
}
pub async fn friend_accept(user_id: i64) -> Result<()> { post("/friends/accept", json!({ "userId": user_id })).await.map(|_| ()) }
pub async fn friend_remove(user_id: i64) -> Result<()> { post("/friends/remove", json!({ "userId": user_id })).await.map(|_| ()) }

// ---------- Packs ----------

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Owner { pub id: i64, pub username: String }

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Member { pub id: i64, pub username: String, pub role: String, pub installed_version: i64, pub installed_at: i64, pub badges: Vec<String> }

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Pack {
    pub id: String,
    pub name: String,
    pub description: String,
    pub minecraft: String,
    pub loader: String,
    pub latest_version: i64,
    pub owner: Owner,
    pub role: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub members: Vec<Member>,
    /// Vom Ersteller erzwungene Mindestversion (0 = keine)
    pub force_version: i64,
    pub force_at: i64,
    pub force_by: String,
}

#[derive(Deserialize)]
struct PackList { packs: Vec<Pack> }
#[derive(Deserialize)]
struct PackOne { pack: Pack }

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct VersionInfo { pub version: i64, pub changelog: String, pub author: String, pub created_at: i64 }

#[derive(Deserialize)]
struct VersionList { versions: Vec<VersionInfo> }

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct VersionFull { pub version: i64, #[serde(default)] pub changelog: String, pub manifest: PackManifest }

pub async fn packs() -> Result<Vec<Pack>> { Ok(get::<PackList>("/packs").await?.packs) }
pub async fn pack(id: &str) -> Result<Pack> { Ok(get::<PackOne>(&format!("/packs/{id}")).await?.pack) }
pub async fn versions(id: &str) -> Result<Vec<VersionInfo>> { Ok(get::<VersionList>(&format!("/packs/{id}/versions")).await?.versions) }
pub async fn latest(id: &str) -> Result<VersionFull> { get(&format!("/packs/{id}/versions/latest")).await }

pub async fn create_pack(name: &str, description: &str, manifest: &PackManifest) -> Result<Pack> {
    let v = post("/packs", json!({ "name": name, "description": description, "manifest": manifest, "changelog": "Erste Version" })).await?;
    Ok(serde_json::from_value::<PackOne>(v)?.pack)
}

pub async fn push_version(id: &str, base: i64, manifest: &PackManifest, changelog: &str) -> Result<i64> {
    let v = post(&format!("/packs/{id}/versions"), json!({ "baseVersion": base, "manifest": manifest, "changelog": changelog })).await?;
    Ok(v["version"].as_i64().unwrap_or(base + 1))
}

/// Meldet dem Server, welche Version hier installiert ist (für die Mitgliederliste)
pub async fn report_installed(id: &str, version: i64) -> Result<()> {
    post(&format!("/packs/{id}/installed"), json!({ "version": version })).await.map(|_| ())
}

/// Besitzer/Editor: neueste Version bei allen Mitgliedern automatisch installieren lassen
pub async fn force_update(id: &str) -> Result<Pack> { Ok(serde_json::from_value::<PackOne>(post(&format!("/packs/{id}/force"), json!({})).await?)?.pack) }

pub async fn delete_pack(id: &str) -> Result<()> { request(reqwest::Method::DELETE, &format!("/packs/{id}"), None).await.map(|_| ()) }
pub async fn add_member(id: &str, user_id: i64) -> Result<()> { post(&format!("/packs/{id}/members"), json!({ "userId": user_id })).await.map(|_| ()) }
pub async fn remove_member(id: &str, user_id: i64) -> Result<()> {
    request(reqwest::Method::DELETE, &format!("/packs/{id}/members/{user_id}"), None).await.map(|_| ())
}
pub async fn set_role(id: &str, user_id: i64, role: &str) -> Result<()> {
    request(reqwest::Method::PUT, &format!("/packs/{id}/members/{user_id}"), Some(json!({ "role": role }))).await.map(|_| ())
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Share { pub code: String, pub url: String }

pub async fn create_share(id: &str, days: i64, max_uses: i64) -> Result<Share> {
    Ok(serde_json::from_value(post(&format!("/packs/{id}/share"), json!({ "expiresInDays": days, "maxUses": max_uses })).await?)?)
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SharePack { pub id: String, pub name: String, pub description: String, pub minecraft: String, pub loader: String, pub owner: String, pub members: i64 }

#[derive(Deserialize)]
struct SharePreview { pack: SharePack }

pub async fn share_preview(code: &str) -> Result<SharePack> { Ok(get::<SharePreview>(&format!("/share/{code}")).await?.pack) }
pub async fn join(code: &str) -> Result<Pack> { Ok(serde_json::from_value::<PackOne>(post(&format!("/share/{code}/join"), json!({})).await?)?.pack) }

// ---------- P2P-Vermittlung ----------

/// Meldet die eigene iroh-ID und welche P2P-Dateien dieser Launcher pro Pack anbietet
pub async fn p2p_announce(node_id: &str, packs: &std::collections::HashMap<String, Vec<String>>) -> Result<()> {
    let list: Vec<Value> = packs.iter().map(|(p, f)| json!({ "packId": p, "files": f })).collect();
    post("/p2p/announce", json!({ "nodeId": node_id, "packs": list })).await.map(|_| ())
}

/// Wer ist online und hat diese Datei? (iroh-ID, Benutzername)
pub async fn p2p_peers(pack_id: &str, sha1: &str) -> Result<Vec<(String, String)>> {
    let v: Value = get(&format!("/packs/{pack_id}/peers?sha1={sha1}")).await?;
    Ok(v["peers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p["nodeId"].as_str()?.to_string(), p["username"].as_str().unwrap_or("?").to_string())))
        .collect())
}

/// iroh-IDs aller Mitglieder eines Packs
pub async fn p2p_nodes(pack_id: &str) -> Result<Vec<String>> {
    let v: Value = get(&format!("/packs/{pack_id}/nodes")).await?;
    Ok(v["nodes"].as_array().into_iter().flatten().filter_map(|n| n.as_str().map(String::from)).collect())
}

// ---------- CurseForge über den Server ----------

pub async fn curseforge_post(path: &str, body: Value) -> Result<Value> { post(&format!("/cf{path}"), body).await }

pub async fn curseforge(path_and_query: &str) -> Result<Value> { request(reqwest::Method::GET, &format!("/cf{path_and_query}"), None).await }
