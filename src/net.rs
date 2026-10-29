//! HTTP-Client und Downloads mit Prüfsummen.

use anyhow::{bail, Context, Result};
use futures::{stream, StreamExt, TryStreamExt};
use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::io::AsyncWriteExt;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(format!("NNNLauncher/{VERSION} (github.com/fojadrachi/NNN-Launcher)"))
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(600))
            .pool_max_idle_per_host(16)
            .build()
            .expect("HTTP-Client")
    })
}

pub async fn get_json(url: &str) -> Result<serde_json::Value> {
    let res = client().get(url).send().await.with_context(|| format!("Keine Verbindung zu {}", host(url)))?;
    if !res.status().is_success() {
        bail!("{} antwortet mit {}", host(url), res.status().as_u16());
    }
    Ok(res.json().await?)
}

pub async fn get_text(url: &str) -> Result<String> {
    let res = client().get(url).send().await.with_context(|| format!("Keine Verbindung zu {}", host(url)))?;
    if !res.status().is_success() {
        bail!("{} antwortet mit {}", host(url), res.status().as_u16());
    }
    Ok(res.text().await?)
}

pub async fn get_bytes(url: &str) -> Result<Vec<u8>> {
    let res = client().get(url).send().await?;
    if !res.status().is_success() {
        bail!("{} antwortet mit {}", host(url), res.status().as_u16());
    }
    Ok(res.bytes().await?.to_vec())
}

fn host(url: &str) -> String {
    url.split('/').nth(2).unwrap_or(url).to_string()
}

pub fn sha1_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha1::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(hex::encode(h.finalize()))
}

pub fn sha1_bytes(data: &[u8]) -> String {
    hex::encode(Sha1::digest(data))
}

#[derive(Clone, Debug)]
pub struct DlItem {
    pub url: String,
    pub path: PathBuf,
    pub sha1: Option<String>,
    pub size: u64,
}

fn needs_download(i: &DlItem) -> bool {
    match std::fs::metadata(&i.path) {
        Err(_) => true,
        Ok(m) if i.size > 0 => m.len() != i.size,
        Ok(_) => match &i.sha1 {
            Some(s) => sha1_file(&i.path).map(|h| !h.eq_ignore_ascii_case(s)).unwrap_or(true),
            None => false,
        },
    }
}

pub async fn download_one(item: &DlItem) -> Result<()> {
    if let Some(dir) = item.path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let tmp = item.path.with_extension("part");
    let mut last = None;
    for attempt in 0..3u64 {
        match try_download(item, &tmp).await {
            Ok(()) => {
                tokio::fs::rename(&tmp, &item.path).await?;
                return Ok(());
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                last = Some(e);
                tokio::time::sleep(std::time::Duration::from_millis(500 * (attempt + 1))).await;
            }
        }
    }
    Err(last.unwrap()).with_context(|| format!("Download fehlgeschlagen: {}", item.url))
}

async fn try_download(item: &DlItem, tmp: &Path) -> Result<()> {
    let res = client().get(&item.url).send().await?;
    if !res.status().is_success() {
        bail!("HTTP {}", res.status().as_u16());
    }
    let mut file = tokio::fs::File::create(tmp).await?;
    let mut hasher = Sha1::new();
    let mut body = res.bytes_stream();
    while let Some(chunk) = body.try_next().await? {
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);
    if let Some(expected) = &item.sha1 {
        let got = hex::encode(hasher.finalize());
        if !got.eq_ignore_ascii_case(expected) {
            bail!("Prüfsumme falsch: {}", item.path.file_name().unwrap_or_default().to_string_lossy());
        }
    }
    Ok(())
}

/// Lädt alle fehlenden oder kaputten Dateien parallel. `progress(fertig, gesamt)`.
pub async fn download_all<F>(items: Vec<DlItem>, parallel: usize, progress: F) -> Result<()>
where
    F: Fn(usize, usize) + Send + Sync + 'static,
{
    let mut seen = std::collections::HashSet::new();
    let todo: Vec<DlItem> = items
        .into_iter()
        .filter(|i| seen.insert(i.path.to_string_lossy().to_lowercase()))
        .filter(needs_download)
        .collect();
    let total = todo.len();
    if total == 0 {
        return Ok(());
    }
    let done = Arc::new(AtomicUsize::new(0));
    let progress = Arc::new(progress);
    progress(0, total);
    stream::iter(todo)
        .map(|item| {
            let done = done.clone();
            let progress = progress.clone();
            async move {
                download_one(&item).await?;
                let d = done.fetch_add(1, Ordering::SeqCst) + 1;
                progress(d, total);
                Ok::<_, anyhow::Error>(())
            }
        })
        .buffer_unordered(parallel.max(1))
        .try_collect::<Vec<_>>()
        .await?;
    Ok(())
}
