//! Eigener Spielstart ohne den offiziellen Launcher: Versionskette auflösen, Dateien laden,
//! Argumente bauen und Java starten. Nutzt denselben Ordner wie der offizielle Launcher.

use crate::instances::{self, Instance};
use crate::session::Account;
use crate::{loaders, minecraft, msauth, net, official, paths};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

const RESOURCES: &str = "https://resources.download.minecraft.net/";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub type Progress = Arc<dyn Fn(String) + Send + Sync>;

/// Zusammengeführte Version (Loader-Profil + Vanilla über "inheritsFrom")
struct Version {
    id: String,
    vanilla_id: String,
    main_class: String,
    kind: String,
    libraries: Vec<Value>,
    game_args: Vec<Value>,
    jvm_args: Vec<Value>,
    legacy_args: Option<String>,
    asset_index: Option<Value>,
    client: Option<Value>,
    logging: Option<Value>,
    java_component: String,
}

struct LibFile {
    path: PathBuf,
    url: Option<String>,
    sha1: Option<String>,
    size: u64,
    native: bool,
}

async fn load_json(id: &str) -> Result<Value> {
    let path = paths::versions().join(id).join(format!("{id}.json"));
    if let Some(v) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
        return Ok(v);
    }
    let v = minecraft::version_json(id).await?;
    std::fs::create_dir_all(paths::versions().join(id))?;
    std::fs::write(&path, serde_json::to_vec(&v)?)?;
    Ok(v)
}

async fn resolve(id: &str) -> Result<Version> {
    let mut chain = vec![load_json(id).await?];
    while let Some(parent) = chain.last().and_then(|j| j["inheritsFrom"].as_str()).map(String::from) {
        if chain.len() > 5 {
            bail!("Zu tiefe inheritsFrom-Kette");
        }
        chain.push(load_json(&parent).await?);
    }
    let pick = |key: &str| chain.iter().find_map(|j| j[key].as_str().map(String::from));
    let pick_obj = |key: &str| chain.iter().map(|j| &j[key]).find(|v| v.is_object()).cloned();

    // Bibliotheken: Kind zuerst, Duplikate (gleiche Gruppe/Artefakt/Classifier) überspringen
    let mut seen = HashSet::new();
    let mut libraries = Vec::new();
    for j in &chain {
        for lib in j["libraries"].as_array().into_iter().flatten() {
            if seen.insert(lib_key(lib["name"].as_str().unwrap_or_default())) {
                libraries.push(lib.clone());
            }
        }
    }
    // Argumente: Eltern zuerst, dann Kind
    let args = |kind: &str| -> Vec<Value> { chain.iter().rev().flat_map(|j| j["arguments"][kind].as_array().cloned().unwrap_or_default()).collect() };

    let root = chain.last().unwrap();
    Ok(Version {
        id: chain[0]["id"].as_str().unwrap_or(id).to_string(),
        vanilla_id: root["id"].as_str().unwrap_or(id).to_string(),
        main_class: pick("mainClass").ok_or_else(|| anyhow!("mainClass fehlt"))?,
        kind: pick("type").unwrap_or_else(|| "release".into()),
        game_args: args("game"),
        jvm_args: args("jvm"),
        legacy_args: pick("minecraftArguments"),
        asset_index: pick_obj("assetIndex"),
        client: root["downloads"]["client"].is_object().then(|| root["downloads"]["client"].clone()),
        logging: pick_obj("logging").map(|l| l["client"].clone()).filter(Value::is_object),
        java_component: pick_obj("javaVersion").and_then(|j| j["component"].as_str().map(String::from)).unwrap_or_else(|| "jre-legacy".into()),
        libraries,
    })
}

/// "group:artifact:version[:classifier][@ext]" → "group:artifact[:classifier]"
fn lib_key(name: &str) -> String {
    let p: Vec<&str> = name.split('@').next().unwrap_or(name).split(':').collect();
    if p.len() >= 4 { format!("{}:{}:{}", p[0], p[1], p[3]) } else { p.iter().take(2).copied().collect::<Vec<_>>().join(":") }
}

fn maven_path(name: &str) -> String {
    let (name, ext) = name.split_once('@').unwrap_or((name, "jar"));
    let p: Vec<&str> = name.split(':').collect();
    if p.len() < 3 {
        return name.to_string();
    }
    let classifier = p.get(3).map(|c| format!("-{c}")).unwrap_or_default();
    format!("{}/{}/{}/{}-{}{classifier}.{ext}", p[0].replace('.', "/"), p[1], p[2], p[1], p[2])
}

/// Regeln aus der Versions-JSON; alle Features (Demo, eigene Auflösung, …) gelten als aus
fn rules_allow(rules: &Value) -> bool {
    let Some(rules) = rules.as_array().filter(|r| !r.is_empty()) else { return true };
    let mut allowed = false;
    for rule in rules {
        let os = &rule["os"];
        let mut hit = os["name"].as_str().is_none_or(|n| n == "windows") && os["arch"].as_str().is_none_or(|a| a == "x86_64");
        if let Some(v) = os["version"].as_str() {
            hit &= regex::Regex::new(v).is_ok_and(|r| r.is_match("10.0"));
        }
        if let Some(f) = rule["features"].as_object() {
            hit &= f.values().all(|want| !want.as_bool().unwrap_or(false));
        }
        if hit {
            allowed = rule["action"] == "allow";
        }
    }
    allowed
}

fn base_url(lib: &Value) -> String {
    let u = lib["url"].as_str().unwrap_or("https://libraries.minecraft.net/");
    if u.ends_with('/') { u.to_string() } else { format!("{u}/") }
}

fn lib_file(root: &Path, path: &str, entry: &Value, native: bool) -> LibFile {
    LibFile {
        path: root.join(path),
        url: entry["url"].as_str().filter(|u| !u.is_empty()).map(String::from),
        sha1: entry["sha1"].as_str().map(String::from),
        size: entry["size"].as_u64().unwrap_or(0),
        native,
    }
}

fn library_files(v: &Version) -> Vec<LibFile> {
    let root = paths::libraries();
    let mut list = Vec::new();
    for lib in &v.libraries {
        if !rules_allow(&lib["rules"]) {
            continue;
        }
        let name = lib["name"].as_str().unwrap_or_default();
        let downloads = &lib["downloads"];

        // Klassische Natives (bis 1.18): eigene Classifier-Jars, die entpackt werden
        if let Some(nat) = lib["natives"]["windows"].as_str() {
            let cls = nat.replace("${arch}", "64");
            let c = &downloads["classifiers"][&cls];
            if c.is_object() {
                list.push(lib_file(&root, c["path"].as_str().unwrap_or_default(), c, true));
            } else {
                let path = maven_path(&format!("{name}:{cls}"));
                list.push(LibFile { path: root.join(&path), url: Some(base_url(lib) + &path), sha1: None, size: 0, native: true });
            }
            if downloads["artifact"].is_null() {
                continue;
            }
        }
        let art = &downloads["artifact"];
        if art.is_object() {
            let path = art["path"].as_str().map(String::from).unwrap_or_else(|| maven_path(name));
            list.push(lib_file(&root, &path, art, false));
        } else if downloads.is_null() {
            // Fabric/Quilt/alte Forge-Einträge: nur Maven-Koordinaten + Repository-URL
            let path = maven_path(name);
            let mut f = lib_file(&root, &path, lib, false);
            f.url = Some(base_url(lib) + &path);
            list.push(f);
        }
    }
    list
}

fn version_jar(v: &Version) -> PathBuf { paths::versions().join(&v.id).join(format!("{}.jar", v.id)) }

fn item(entry: &Value, path: PathBuf) -> net::DlItem {
    net::DlItem {
        url: entry["url"].as_str().unwrap_or_default().to_string(),
        path,
        sha1: entry["sha1"].as_str().map(String::from),
        size: entry["size"].as_u64().unwrap_or(0),
    }
}

async fn fetch(label: &str, items: Vec<net::DlItem>, parallel: usize, progress: &Progress) -> Result<()> {
    let (l, p) = (label.to_string(), progress.clone());
    net::download_all(items, parallel, move |done, total| p(format!("{l} {done}/{total}"))).await.with_context(|| label.to_string())
}

async fn ensure_files(v: &Version, progress: &Progress) -> Result<PathBuf> {
    progress("Minecraft-Dateien prüfen".into());

    // Client-Jar der Vanilla-Version; für Loader-Profile unter deren ID kopiert (wie der offizielle Launcher)
    let vanilla = paths::versions().join(&v.vanilla_id).join(format!("{}.jar", v.vanilla_id));
    if let Some(c) = &v.client {
        fetch("Minecraft", vec![item(c, vanilla.clone())], 1, progress).await?;
    }
    let jar = version_jar(v);
    if jar != vanilla && std::fs::metadata(&jar).map(|m| m.len()).ok() != std::fs::metadata(&vanilla).map(|m| m.len()).ok() {
        std::fs::create_dir_all(jar.parent().unwrap())?;
        std::fs::copy(&vanilla, &jar).context("Client-Jar kopieren")?;
    }

    let libs = library_files(v);
    if let Some(l) = libs.iter().find(|l| l.url.is_none() && !l.path.exists()) {
        bail!("Bibliothek fehlt (Loader neu installieren): {}", l.path.file_name().unwrap_or_default().to_string_lossy());
    }
    let items = libs.iter().filter_map(|l| Some(net::DlItem { url: l.url.clone()?, path: l.path.clone(), sha1: l.sha1.clone(), size: l.size })).collect();
    fetch("Bibliotheken", items, 12, progress).await?;

    ensure_assets(v, progress).await?;

    if let Some(l) = &v.logging {
        let file = &l["file"];
        let path = paths::assets().join("log_configs").join(file["id"].as_str().unwrap_or("client.xml"));
        fetch("Log-Konfiguration", vec![item(file, path)], 1, progress).await?;
    }
    minecraft::ensure_java(&v.java_component, &*progress.clone()).await
}

fn asset_name(v: &Version) -> String { v.asset_index.as_ref().and_then(|a| a["id"].as_str()).unwrap_or("legacy").to_string() }

async fn ensure_assets(v: &Version, progress: &Progress) -> Result<()> {
    let Some(index) = &v.asset_index else { return Ok(()) };
    let id = asset_name(v);
    let root = paths::assets();
    let index_path = root.join("indexes").join(format!("{id}.json"));
    fetch("Asset-Index", vec![item(index, index_path.clone())], 1, progress).await?;
    let data: Value = serde_json::from_slice(&std::fs::read(&index_path)?)?;
    let objects = data["objects"].as_object().ok_or_else(|| anyhow!("Asset-Index ist leer"))?;

    let object = |hash: &str| root.join("objects").join(&hash[..2]).join(hash);
    let mut items = Vec::with_capacity(objects.len());
    for o in objects.values() {
        let hash = o["hash"].as_str().filter(|h| h.len() > 2).ok_or_else(|| anyhow!("Asset-Index ist beschädigt"))?;
        items.push(net::DlItem { url: format!("{RESOURCES}{}/{hash}", &hash[..2]), path: object(hash), sha1: Some(hash.to_string()), size: o["size"].as_u64().unwrap_or(0) });
    }
    fetch("Assets", items, 24, progress).await?;

    if data["virtual"] == true || data["map_to_resources"] == true {
        let virt = root.join("virtual").join(&id);
        for (name, o) in objects {
            let target = paths::safe_join(&virt, name)?;
            if !target.exists() {
                std::fs::create_dir_all(target.parent().unwrap())?;
                std::fs::copy(object(o["hash"].as_str().unwrap_or_default()), &target)?;
            }
        }
    }
    Ok(())
}

fn game_assets_dir(v: &Version) -> PathBuf {
    let virt = paths::assets().join("virtual").join(asset_name(v));
    if virt.exists() { virt } else { paths::assets() }
}

fn extract_natives(libs: &[LibFile], target: &Path) -> Result<()> {
    let _ = std::fs::remove_dir_all(target);
    std::fs::create_dir_all(target)?;
    for lib in libs.iter().filter(|l| l.native && l.path.exists()) {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&lib.path)?)?;
        for i in 0..zip.len() {
            let mut e = zip.by_index(i)?;
            let name = e.name().to_string();
            if name.to_uppercase().starts_with("META-INF") || name.ends_with('/') {
                continue;
            }
            let dest = paths::safe_join(target, &name)?;
            std::fs::create_dir_all(dest.parent().unwrap())?;
            std::io::copy(&mut e, &mut std::fs::File::create(dest)?)?;
        }
    }
    Ok(())
}

fn subst(s: &str, vars: &HashMap<&str, String>) -> String {
    let re = regex::Regex::new(r"\$\{(\w+)\}").unwrap();
    re.replace_all(s, |c: &regex::Captures| vars.get(&c[1]).cloned().unwrap_or_else(|| c[0].to_string())).into_owned()
}

fn expand(list: &[Value], vars: &HashMap<&str, String>) -> Vec<String> {
    let mut out = Vec::new();
    for node in list {
        if let Some(s) = node.as_str() {
            out.push(subst(s, vars));
        } else if rules_allow(&node["rules"]) {
            match &node["value"] {
                Value::Array(a) => out.extend(a.iter().filter_map(Value::as_str).map(|s| subst(s, vars))),
                Value::String(s) => out.push(subst(s, vars)),
                _ => {}
            }
        }
    }
    out
}

fn split_args(s: &str) -> Vec<String> {
    let re = regex::Regex::new(r#""([^"]*)"|(\S+)"#).unwrap();
    re.captures_iter(s).map(|c| c.get(1).or(c.get(2)).map(|m| m.as_str().to_string()).unwrap_or_default()).collect()
}

/// Das laufende Spiel; die Ausgabe landet in logs\<Instanz>-latest.log
pub struct Running {
    child: tokio::process::Child,
    pub log: PathBuf,
}

impl Running {
    /// Wartet, bis das Spiel beendet ist, und gibt den Exit-Code zurück
    pub async fn wait(mut self) -> i32 { self.child.wait().await.ok().and_then(|s| s.code()).unwrap_or(-1) }

    /// Nur für den Selbsttest
    #[cfg(debug_assertions)]
    pub async fn kill(&mut self) { let _ = self.child.kill().await; }
}

fn pipe_log<R: AsyncRead + Unpin + Send + 'static>(from: R, log: Arc<Mutex<std::fs::File>>, secret: String) {
    tokio::spawn(async move {
        let mut r = BufReader::new(from);
        let mut buf = Vec::new();
        while r.read_until(b'\n', &mut buf).await.is_ok_and(|n| n > 0) {
            let mut line = String::from_utf8_lossy(&buf).into_owned();
            if !secret.is_empty() {
                line = line.replace(&secret, "<token>");
            }
            let _ = std::io::Write::write_all(&mut *log.lock().unwrap(), line.as_bytes());
            buf.clear();
        }
    });
}

pub async fn launch(inst: &Instance, acc: &Account, memory_mb: u32, extra: &str, progress: Progress) -> Result<Running> {
    let m = &inst.manifest;
    progress("Loader vorbereiten".into());
    let version_id = loaders::install(&m.loader, &m.minecraft, m.loader_version.as_deref(), &*progress).await?;
    let v = resolve(&version_id).await?;
    let java = ensure_files(&v, &progress).await?;

    progress("Starten".into());
    let game = inst.game_dir();
    std::fs::create_dir_all(&game)?;
    let natives = paths::natives().join(&inst.id);
    let libs = library_files(&v);
    extract_natives(&libs, &natives)?;

    let mut seen = HashSet::new();
    let mut cp: Vec<String> = libs.iter().filter(|l| !l.native).map(|l| l.path.display().to_string()).filter(|p| seen.insert(p.to_lowercase())).collect();
    cp.push(version_jar(&v).display().to_string());
    let classpath = cp.join(";");

    let token = acc.access_token.clone().unwrap_or_else(|| "0".into());
    let vars: HashMap<&str, String> = HashMap::from([
        ("auth_player_name", acc.name.clone()),
        ("version_name", v.id.clone()),
        ("game_directory", game.display().to_string()),
        ("assets_root", paths::assets().display().to_string()),
        ("game_assets", game_assets_dir(&v).display().to_string()),
        ("assets_index_name", asset_name(&v)),
        ("auth_uuid", acc.id.clone()),
        ("auth_access_token", token.clone()),
        ("auth_session", token.clone()),
        ("auth_xuid", acc.xuid.clone().unwrap_or_else(|| "0".into())),
        ("clientid", msauth::CLIENT_ID.to_string()),
        ("user_type", if acc.kind == "msa" { "msa" } else { "legacy" }.to_string()),
        ("user_properties", "{}".to_string()),
        ("version_type", v.kind.clone()),
        ("natives_directory", natives.display().to_string()),
        ("launcher_name", "NNN Launcher".to_string()),
        ("launcher_version", net::VERSION.to_string()),
        ("classpath", classpath.clone()),
        ("classpath_separator", ";".to_string()),
        ("library_directory", paths::libraries().display().to_string()),
    ]);

    let mut args: Vec<String> = vec!["-Dstdout.encoding=UTF-8".into(), "-Dstderr.encoding=UTF-8".into(), "-Dfile.encoding=UTF-8".into()];
    args.extend(split_args(&official::java_args(inst.memory_mb.unwrap_or(memory_mb), extra)));
    if v.jvm_args.is_empty() {
        args.extend([format!("-Djava.library.path={}", natives.display()), "-cp".into(), classpath]);
    } else {
        args.extend(expand(&v.jvm_args, &vars));
    }
    if let Some(l) = &v.logging {
        let path = paths::assets().join("log_configs").join(l["file"]["id"].as_str().unwrap_or("client.xml"));
        args.push(l["argument"].as_str().unwrap_or_default().replace("${path}", &path.display().to_string()));
    }
    args.push(v.main_class.clone());
    if !v.game_args.is_empty() {
        args.extend(expand(&v.game_args, &vars));
    } else if let Some(legacy) = &v.legacy_args {
        args.extend(split_args(legacy).iter().map(|a| subst(a, &vars)));
    }

    let log_path = paths::logs().join(format!("{}-latest.log", inst.id));
    let mut log = std::fs::File::create(&log_path)?;
    std::io::Write::write_all(&mut log, format!("[NNN] Starte {} als {} mit {}\n", v.id, acc.name, java.display()).as_bytes())?;

    let mut child = tokio::process::Command::new(&java)
        .args(&args)
        .current_dir(&game)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("Java konnte nicht gestartet werden")?;
    let log = Arc::new(Mutex::new(log));
    let secret = acc.access_token.clone().unwrap_or_default();
    if let Some(o) = child.stdout.take() {
        pipe_log(o, log.clone(), secret.clone());
    }
    if let Some(e) = child.stderr.take() {
        pipe_log(e, log, secret);
    }

    let mut i = instances::get(&inst.id)?;
    i.last_played = instances::now();
    instances::save(&i)?;
    Ok(Running { child, log: log_path })
}
