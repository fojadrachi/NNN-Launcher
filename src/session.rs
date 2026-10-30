//! Anmeldung beim NNN-Server, per Windows-DPAPI verschlüsselt (nur dieser Windows-Nutzer kann sie lesen).
//! Gleiche Datei wie die C#-Version (accounts.dat), dadurch bleibt man nach dem Umstieg angemeldet.

use crate::paths;
use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct User {
    pub id: i64,
    pub username: String,
    /// Vom Server vergeben, z. B. "owner", "beta"
    pub badges: Vec<String>,
}

/// Minecraft-Konto (Microsoft). Tokens liegen nur verschlüsselt in accounts.dat.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Account {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    pub access_expires: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xuid: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
struct Store {
    #[serde(deserialize_with = "readable_accounts")]
    accounts: Vec<Account>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    server_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    server_user: Option<User>,
}

/// Unlesbare Konten (z. B. aus der C#-Version) überspringen, damit die Server-Anmeldung erhalten bleibt
fn readable_accounts<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Account>, D::Error> {
    let list = Option::<Vec<serde_json::Value>>::deserialize(d).unwrap_or_default().unwrap_or_default();
    Ok(list.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
}

fn file() -> std::path::PathBuf { paths::root().join("accounts.dat") }

fn load_store() -> Store {
    std::fs::read(file()).ok().and_then(|b| unprotect(&b)).and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or_default()
}

/// Entwickler-Builds gegen einen Testserver (NNN_SERVER) fassen die echte Anmeldung nie an
fn test_server() -> bool {
    #[cfg(debug_assertions)]
    if std::env::var_os("NNN_SERVER").is_some() {
        return true;
    }
    false
}

pub fn load() -> Option<(String, User)> {
    if test_server() {
        return None;
    }
    let s = load_store();
    Some((s.server_token?, s.server_user?))
}

pub fn save(token: Option<&str>, user: Option<&User>) {
    if test_server() {
        return;
    }
    let mut s = load_store();
    s.server_token = token.map(String::from);
    s.server_user = user.cloned();
    if let Ok(raw) = serde_json::to_vec(&s) {
        if let Some(enc) = protect(&raw) {
            let _ = std::fs::write(file(), enc);
        }
    }
}

fn write_store(s: &Store) {
    if test_server() {
        return;
    }
    if let Ok(raw) = serde_json::to_vec(s) {
        if let Some(enc) = protect(&raw) {
            let _ = std::fs::write(file(), enc);
        }
    }
}

/// Microsoft-Konten und das aktive Konto
pub fn accounts() -> (Vec<Account>, Option<String>) {
    if test_server() {
        return Default::default();
    }
    let s = load_store();
    (s.accounts.into_iter().filter(|a| a.kind == "msa").collect(), s.selected_id)
}

pub fn upsert_account(acc: Account) {
    let mut s = load_store();
    s.accounts.retain(|a| a.id != acc.id);
    s.selected_id = Some(acc.id.clone());
    s.accounts.push(acc);
    write_store(&s);
}

pub fn remove_account(id: &str) {
    let mut s = load_store();
    s.accounts.retain(|a| a.id != id);
    if s.selected_id.as_deref() == Some(id) {
        s.selected_id = s.accounts.iter().find(|a| a.kind == "msa").map(|a| a.id.clone());
    }
    write_store(&s);
}

pub fn select_account(id: &str) {
    let mut s = load_store();
    if s.accounts.iter().any(|a| a.id == id) {
        s.selected_id = Some(id.to_string());
        write_store(&s);
    }
}

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
}

fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    // SAFETY: von DPAPI allokierter Puffer der angegebenen Länge, danach freigegeben
    unsafe {
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as _);
        v
    }
}

fn protect(data: &[u8]) -> Option<Vec<u8>> {
    let input = blob(data);
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: gültige Ein-/Ausgabe-Blobs, keine optionalen Parameter
    let ok = unsafe { CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 0, &mut out) };
    (ok != 0).then(|| take(out))
}

fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    let input = blob(data);
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: wie oben
    let ok = unsafe { CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 0, &mut out) };
    (ok != 0).then(|| take(out))
}
