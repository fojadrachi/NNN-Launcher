//! Einstellungen (settings.json, kompatibel zur C#-Version) und Speicher für Benachrichtigungen.

use crate::paths;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub memory_mb: u32,
    pub jvm_args: String,
    pub official_launcher_path: String,
    pub close_on_launch: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { memory_mb: 4096, jvm_args: String::new(), official_launcher_path: String::new(), close_on_launch: false }
    }
}

fn file() -> std::path::PathBuf { paths::root().join("settings.json") }

pub fn load() -> Settings {
    std::fs::read(file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(s: &Settings) {
    if let Ok(b) = serde_json::to_vec_pretty(s) {
        let _ = std::fs::write(file(), b);
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Event {
    pub key: String,
    pub kind: String,
    pub title: String,
    pub user: String,
    pub time: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Outgoing {
    pub id: i64,
    pub username: String,
}

/// Gelesen-Status, Verlauf und zuletzt gesendete Anfragen – pro Konto
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct NotifStore {
    pub seen: Vec<String>,
    pub dismissed: Vec<String>,
    pub events: Vec<Event>,
    pub outgoing: Vec<Outgoing>,
}

fn notif_file(user: i64) -> std::path::PathBuf { paths::root().join(format!("notifications-{user}.json")) }

pub fn load_notifs(user: i64) -> NotifStore {
    std::fs::read(notif_file(user)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_notifs(user: i64, n: &NotifStore) {
    let mut n = n.clone();
    let cut = |v: &mut Vec<String>| {
        let len = v.len();
        if len > 300 {
            v.drain(0..len - 300);
        }
    };
    cut(&mut n.seen);
    cut(&mut n.dismissed);
    n.events.truncate(40);
    if let Ok(b) = serde_json::to_vec(&n) {
        let _ = std::fs::write(notif_file(user), b);
    }
}
