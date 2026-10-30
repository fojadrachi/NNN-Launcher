//! Dateien direkt zwischen Launchern tauschen (P2P über iroh/QUIC).
//!
//! - Jeder Launcher hat eine feste iroh-ID (Schlüssel in `p2p.key`).
//! - Der NNN-Server vermittelt nur: wer hat welche Datei, wer ist online. Dateien laufen nie über ihn.
//! - Ein Launcher gibt eine Datei nur an iroh-IDs heraus, die Mitglied desselben Packs sind.
//! - iroh baut die Verbindung direkt auf (Hole Punching); klappt das nicht, läuft sie verschlüsselt
//!   über ein öffentliches Relay. Inhalte sind immer Ende-zu-Ende verschlüsselt.

use crate::{api, instances, net, paths};
use anyhow::{anyhow, bail, Context, Result};
use futures::StreamExt;
use iroh::endpoint::{presets, Connection};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointId, SecretKey};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::OnceCell;

const ALPN: &[u8] = b"nnn-launcher/file/1";
/// Adresse einer Datei im Manifest: p2p://<packId>/<sha1>
pub const SCHEME: &str = "p2p://";

static NODE: OnceCell<Router> = OnceCell::const_new();
/// Dateien, die dieser Launcher anbieten kann: (Pack, SHA-1) → Pfad
static OFFER: RwLock<Option<HashMap<(String, String), PathBuf>>> = RwLock::new(None);
/// Erlaubte iroh-IDs pro Pack (Mitglieder), vom Server
static ALLOWED: RwLock<Option<HashMap<String, (HashSet<String>, std::time::Instant)>>> = RwLock::new(None);

pub fn url(pack_id: &str, sha1: &str) -> String { format!("{SCHEME}{pack_id}/{sha1}") }

fn parse(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix(SCHEME)?;
    let (pack, sha1) = rest.split_once('/')?;
    (pack.len() == 10 && sha1.len() == 40).then(|| (pack.to_string(), sha1.to_lowercase()))
}

fn secret_key() -> SecretKey {
    #[allow(unused_mut)]
    let mut file = paths::root().join("p2p.key");
    // Entwickler-Builds: zweiter Testprozess mit eigenem Schlüssel
    #[cfg(debug_assertions)]
    if let Ok(f) = std::env::var("NNN_P2P_KEY") {
        file = PathBuf::from(f);
    }
    if let Ok(b) = std::fs::read(&file) {
        if let Ok(arr) = <[u8; 32]>::try_from(b.as_slice()) {
            return SecretKey::from_bytes(&arr);
        }
    }
    let key = SecretKey::generate();
    let _ = std::fs::write(&file, key.to_bytes());
    key
}

/// Startet den P2P-Knoten (einmal pro Programmlauf)
pub async fn start() -> Result<&'static Router> {
    NODE.get_or_try_init(|| async {
        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(secret_key())
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .map_err(|e| anyhow!("P2P konnte nicht starten: {e}"))?;
        Ok::<_, anyhow::Error>(Router::builder(endpoint).accept(ALPN, Provider).spawn())
    })
    .await
}

pub async fn node_id() -> Result<String> { Ok(start().await?.endpoint().id().to_string()) }

// ---------- Anbieten ----------

/// Sammelt alle P2P-Dateien der Instanzen, die hier auf der Platte liegen, und meldet sie dem Server
pub async fn announce() -> Result<usize> {
    if !api::logged_in() {
        return Ok(0);
    }
    let mut offer: HashMap<(String, String), PathBuf> = HashMap::new();
    let mut per_pack: HashMap<String, Vec<String>> = HashMap::new();
    for inst in instances::list() {
        let Some(pack) = inst.pack_id.clone() else { continue };
        for k in instances::Kind::ALL {
            for m in inst.manifest.list(k) {
                let Some((p, sha1)) = parse(&m.url) else { continue };
                let Some(path) = instances::existing_path(&inst.id, k, &m.file_name) else { continue };
                offer.insert((p.clone(), sha1.clone()), path);
                per_pack.entry(pack.clone()).or_default().push(sha1);
            }
        }
    }
    let count = offer.len();
    *OFFER.write().unwrap() = Some(offer);
    let id = node_id().await?;
    api::p2p_announce(&id, &per_pack).await?;
    Ok(count)
}

async fn allowed(pack: &str, remote: &str) -> bool {
    let fresh = |force: bool| {
        ALLOWED.read().unwrap().as_ref().and_then(|m| m.get(pack).cloned()).filter(|(_, at)| !force && at.elapsed() < Duration::from_secs(120))
    };
    if let Some((set, _)) = fresh(false) {
        if set.contains(remote) {
            return true;
        }
    }
    // Neu laden – vielleicht ist der Freund gerade erst beigetreten
    match api::p2p_nodes(pack).await {
        Ok(nodes) => {
            let set: HashSet<String> = nodes.into_iter().collect();
            let ok = set.contains(remote);
            ALLOWED.write().unwrap().get_or_insert_with(HashMap::new).insert(pack.to_string(), (set, std::time::Instant::now()));
            ok
        }
        Err(_) => false,
    }
}

#[derive(Debug, Clone)]
struct Provider;

impl ProtocolHandler for Provider {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote = connection.remote_id().to_string();
        let (mut send, mut recv) = connection.accept_bi().await?;
        // Anfrage: eine Zeile "<packId> <sha1>"
        let mut line = String::new();
        let mut reader = BufReader::new(&mut recv);
        let _ = reader.read_line(&mut line).await;
        let mut parts = line.split_whitespace();
        let (pack, sha1) = (parts.next().unwrap_or_default().to_string(), parts.next().unwrap_or_default().to_lowercase());

        let path = OFFER.read().unwrap().as_ref().and_then(|o| o.get(&(pack.clone(), sha1.clone())).cloned());
        let reply = match (&path, allowed(&pack, &remote).await) {
            (_, false) => Err("kein Mitglied dieses Packs"),
            (None, _) => Err("Datei nicht vorhanden"),
            (Some(p), true) => tokio::fs::File::open(p).await.map_err(|_| "Datei nicht lesbar"),
        };
        match reply {
            Ok(mut file) => {
                let size = file.metadata().await.map(|m| m.len()).unwrap_or(0);
                AsyncWriteExt::write_all(&mut send, format!("OK {size}\n").as_bytes()).await?;
                tokio::io::copy(&mut file, &mut send).await?;
            }
            Err(msg) => {
                AsyncWriteExt::write_all(&mut send, format!("ERR {msg}\n").as_bytes()).await?;
            }
        }
        send.finish()?;
        connection.closed().await;
        Ok(())
    }
}

// ---------- Herunterladen ----------

/// Lädt eine P2P-Datei von einem Freund, der sie hat und online ist
pub async fn fetch(item: &net::DlItem) -> Result<()> {
    let (pack, sha1) = parse(&item.url).ok_or_else(|| anyhow!("Ungültige P2P-Adresse"))?;
    // Liegt die Datei auf dem Server, muss niemand online sein
    if from_server(&pack, &sha1, item).await.unwrap_or(false) {
        return Ok(());
    }
    let router = start().await?;
    let peers = api::p2p_peers(&pack, &sha1).await?;
    let name = item.path.file_name().unwrap_or_default().to_string_lossy().to_string();
    if peers.is_empty() {
        bail!("{name}: gerade ist kein Freund online, der die Datei hat");
    }
    let mut last = None;
    for (node, user) in peers {
        match tokio::time::timeout(Duration::from_secs(600), fetch_from(router.endpoint(), &node, &pack, &sha1, item)).await {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(e)) => last = Some(anyhow!("{name} von {user}: {e:#}")),
            Err(_) => last = Some(anyhow!("{name} von {user}: Zeitüberschreitung")),
        }
    }
    Err(last.unwrap())
}

async fn from_server(pack: &str, sha1: &str, item: &net::DlItem) -> Result<bool> {
    let Some(res) = api::download_file(pack, sha1).await? else { return Ok(false) };
    if let Some(dir) = item.path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let tmp = net::part_path(&item.path);
    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut hasher = <sha1::Sha1 as sha1::Digest>::new();
    let mut body = res.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        sha1::Digest::update(&mut hasher, &chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);
    if hex::encode(sha1::Digest::finalize(hasher)) != sha1 {
        let _ = tokio::fs::remove_file(&tmp).await;
        bail!("Prüfsumme falsch");
    }
    tokio::fs::rename(&tmp, &item.path).await?;
    Ok(true)
}

async fn fetch_from(endpoint: &Endpoint, node: &str, pack: &str, sha1: &str, item: &net::DlItem) -> Result<()> {
    let id: EndpointId = node.parse().map_err(|_| anyhow!("ungültige Knoten-ID"))?;
    let conn = tokio::time::timeout(Duration::from_secs(25), endpoint.connect(id, ALPN))
        .await
        .map_err(|_| anyhow!("keine Verbindung"))?
        .map_err(|e| anyhow!("keine Verbindung ({e})"))?;
    let (mut send, recv) = conn.open_bi().await.map_err(|e| anyhow!("{e}"))?;
    send.write_all(format!("{pack} {sha1}\n").as_bytes()).await?;
    send.finish().map_err(|e| anyhow!("{e}"))?;

    let mut reader = BufReader::new(recv);
    let mut header = String::new();
    reader.read_line(&mut header).await?;
    let header = header.trim();
    if let Some(msg) = header.strip_prefix("ERR ") {
        bail!("{msg}");
    }
    let size: u64 = header.strip_prefix("OK ").and_then(|s| s.parse().ok()).context("ungültige Antwort")?;

    if let Some(dir) = item.path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let tmp = crate::net::part_path(&item.path);
    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut hasher = <sha1::Sha1 as sha1::Digest>::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut got = 0u64;
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        sha1::Digest::update(&mut hasher, &buf[..n]);
        file.write_all(&buf[..n]).await?;
        got += n as u64;
    }
    file.flush().await?;
    drop(file);
    conn.close(0u32.into(), b"danke");
    let hash = hex::encode(sha1::Digest::finalize(hasher));
    if got != size || hash != sha1 {
        let _ = tokio::fs::remove_file(&tmp).await;
        bail!("Übertragung unvollständig oder Prüfsumme falsch");
    }
    tokio::fs::rename(&tmp, &item.path).await?;
    Ok(())
}
