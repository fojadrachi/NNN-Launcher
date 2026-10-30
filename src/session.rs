//! Anmeldung beim NNN-Server, per Windows-DPAPI verschlüsselt (nur dieser Windows-Nutzer kann sie lesen).
//! Gleiche Datei wie die C#-Version (accounts.dat), dadurch bleibt man nach dem Umstieg angemeldet.

use crate::paths;
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
struct Store {
    // Felder der alten Version unverändert durchreichen
    accounts: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    server_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    server_user: Option<User>,
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
    if s.accounts.is_null() {
        s.accounts = Value::Array(vec![]);
    }
    if let Ok(raw) = serde_json::to_vec(&s) {
        if let Some(enc) = protect(&raw) {
            let _ = std::fs::write(file(), enc);
        }
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
