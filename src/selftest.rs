//! Selbsttest ohne Oberfläche: `nnn-launcher.exe --selftest` – Ergebnis in logs\selftest.log.
//! Legt eine Fabric-Instanz an, installiert Sodium samt Abhängigkeiten, bereitet den Start
//! über den offiziellen Launcher vor (ohne ihn zu öffnen) und räumt danach wieder auf.

use crate::instances::Kind;
use crate::{instances, loaders, minecraft, mods, net, official, paths, scan};
use std::io::Write;

pub fn run() -> i32 {
    std::env::set_var("NNN_DRY_LAUNCH", "1");
    let rt = tokio::runtime::Runtime::new().expect("Tokio");
    let mut log = std::fs::File::create(paths::logs().join("selftest.log")).expect("Log");
    let mut w = |s: String| {
        let _ = writeln!(log, "{} {s}", chrono::Local::now().format("%H:%M:%S"));
    };
    let res: anyhow::Result<()> = rt.block_on(async {
        let mc = minecraft::release_versions().await?;
        w(format!("Minecraft-Versionen: {} (neueste {})", mc.len(), mc.first().cloned().unwrap_or_default()));
        let fabric = loaders::list("fabric", "1.20.1").await?;
        let lv = fabric.iter().find(|l| !l.label.contains("beta")).map(|l| l.version.clone()).unwrap_or_default();
        w(format!("Fabric 1.20.1: {} Versionen, gewählt {lv}", fabric.len()));
        let forge = loaders::list("forge", "1.20.1").await?;
        w(format!("Forge 1.20.1: {} Versionen, erste {}", forge.len(), forge.first().map(|l| l.label.clone()).unwrap_or_default()));
        let neo = loaders::list("neoforge", "1.21.1").await?;
        w(format!("NeoForge 1.21.1: {} Versionen, erste {}", neo.len(), neo.first().map(|l| l.label.clone()).unwrap_or_default()));

        let inst = instances::create("NNN Selbsttest", "1.20.1", "fabric", Some(lv))?;
        w(format!("Instanz angelegt: {}", inst.id));
        let (hits, total) = mods::search(Kind::Mod, "modrinth", "sodium", Some(&inst), 0).await?;
        w(format!("Suche 'sodium': {total} Treffer, erster: {}", hits.first().map(|h| h.title.clone()).unwrap_or_default()));
        let hit = hits.first().ok_or_else(|| anyhow::anyhow!("kein Treffer"))?;
        let list = mods::resolve_with_deps(Kind::Mod, &hit.source, &hit.id, &hit.title, hit.icon.clone(), &inst).await?;
        w(format!("Aufgelöst: {}", list.iter().map(|m| format!("{} ({})", m.name, m.file_name)).collect::<Vec<_>>().join(", ")));
        let mut last = inst.clone();
        for m in list {
            last = instances::add_item(&inst.id, Kind::Mod, m).await?;
        }
        // Texturepack und Shader
        for (kind, q) in [(Kind::ResourcePack, "faithful"), (Kind::Shader, "complementary")] {
            let (hits, total) = mods::search(kind, "modrinth", q, Some(&last), 0).await?;
            let hit = hits.first().ok_or_else(|| anyhow::anyhow!("kein {} gefunden", kind.label()))?;
            let list = mods::resolve_with_deps(kind, &hit.source, &hit.id, &hit.title, hit.icon.clone(), &last).await?;
            w(format!("{} '{q}': {total} Treffer, installiere {} ({})", kind.label(), list[0].name, list[0].file_name));
            last = instances::add_item(&inst.id, kind, list.into_iter().next().unwrap()).await?;
            let files: Vec<String> = std::fs::read_dir(last.game_dir().join(kind.folder()))?.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
            w(format!("  {}/: {}", kind.folder(), files.join(", ")));
        }
        w(format!("Shader-Loader vorhanden: {} (erwartet: nein, nur Sodium)", last.has_shader_loader()));
        let files: Vec<String> = std::fs::read_dir(last.game_dir().join("mods"))?.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        w(format!("Im mods-Ordner: {}", files.join(", ")));

        let r = official::launch(&last, 4096, "", "", &|s| println!("{s}")).await?;
        w(format!("Start vorbereitet: {}", r.profile));
        let profiles = std::fs::read_to_string(paths::game().join("launcher_profiles.json"))?;
        let v: serde_json::Value = serde_json::from_str(&profiles)?;
        let p = &v["profiles"][format!("nnn-{}", inst.id)];
        w(format!("Profil: name={} version={} gameDir={}", p["name"], p["lastVersionId"], p["gameDir"]));

        // Manifest-Hash wie in der C#-Version?
        for i in instances::list() {
            if let Some(h) = &i.published_hash {
                w(format!("Hash {}: {}", i.id, if *h == instances::manifest_hash(&i.manifest) { "passt" } else { "ABWEICHUNG" }));
            }
        }

        // Ordner-Scan: Dateien von Hand hineinlegen
        let scan_inst = instances::create("NNN Scan-Test", "1.20.1", "fabric", Some("0.20.1".into()))?;
        let mods_dir = scan_inst.game_dir().join("mods");
        net::download_one(&net::DlItem {
            url: last.manifest.mods[0].url.clone(),
            path: mods_dir.join("von-hand-kopiert.jar"),
            sha1: None,
            size: 0,
        })
        .await?;
        {
            // Selbst gebauter Mod mit fabric.mod.json und Icon
            let f = std::fs::File::create(mods_dir.join("mein-mod-1.0.jar"))?;
            let mut z = zip::ZipWriter::new(f);
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("fabric.mod.json", o)?;
            z.write_all(br#"{"schemaVersion":1,"id":"meinmod","name":"Mein Test-Mod","version":"1.0.3"}"#)?;
            z.finish()?;
        }
        let r = scan::scan(&scan_inst).await?;
        for f in &r.found {
            w(format!(
                "Scan: {} → {}",
                f.file_name,
                match &f.entry {
                    Some(e) => format!("erkannt: {} {} ({})", e.name, e.version_name.clone().unwrap_or_default(), e.source),
                    None => format!("nicht erkannt, aus Datei: „{}“ {}", f.meta_name, f.meta_version),
                }
            ));
        }
        let adopt: Vec<(Kind, instances::ModEntry)> = r.found.iter().filter_map(|f| Some((f.kind?, f.entry.clone()?))).collect();
        let s = instances::adopt(&scan_inst.id, adopt, &r.missing)?;
        w(format!("Übernommen ins Pack: {}", s.manifest.mods.iter().map(|m| format!("{} ({})", m.name, m.file_name)).collect::<Vec<_>>().join(", ")));
        // Ausschalten: umbenennen, Scan darf es nicht als gelöscht sehen, Sync lässt es aus
        instances::set_enabled(&scan_inst.id, Kind::Mod, "von-hand-kopiert.jar", false)?;
        let off_ok = mods_dir.join("von-hand-kopiert.jar.disabled").exists() && !mods_dir.join("von-hand-kopiert.jar").exists();
        let r_off = scan::scan(&s).await?;
        let synced = instances::apply_manifest(&scan_inst.id, s.manifest.clone(), 1, None, |_, _| {}).await?;
        w(format!(
            "Ausgeschaltet: umbenannt={off_ok}, Scan hält es für gelöscht={}, nach Sync noch aus={}, .jar neu geladen={}",
            !r_off.missing.is_empty(),
            instances::is_disabled(&scan_inst.id, Kind::Mod, "von-hand-kopiert.jar"),
            mods_dir.join("von-hand-kopiert.jar").exists()
        ));
        instances::set_enabled(&scan_inst.id, Kind::Mod, "von-hand-kopiert.jar", true)?;
        w(format!("Wieder eingeschaltet: {}", mods_dir.join("von-hand-kopiert.jar").exists() && !instances::is_disabled(&scan_inst.id, Kind::Mod, "von-hand-kopiert.jar")));
        let s = synced;
        std::fs::remove_file(mods_dir.join("von-hand-kopiert.jar"))?;
        let r2 = scan::scan(&s).await?;
        let s2 = instances::adopt(&scan_inst.id, vec![], &r2.missing)?;
        w(format!("Nach Löschen: fehlend erkannt {:?}, Mods im Pack jetzt {}", r2.missing.iter().map(|m| &m.1).collect::<Vec<_>>(), s2.manifest.mods.len()));
        instances::delete(&scan_inst.id)?;

        instances::delete(&inst.id)?;
        w("Aufgeräumt".into());
        Ok(())
    });
    match res {
        Ok(()) => {
            w("OK".into());
            0
        }
        Err(e) => {
            w(format!("FEHLER: {e:#}"));
            1
        }
    }
}

/// Entwickler-Test gegen einen lokalen Server: eigene Datei hochladen, zweites Konto lädt sie ohne P2P-Anbieter
#[cfg(debug_assertions)]
pub fn r2_test() -> i32 {
    use crate::{api, p2p};
    let rt = tokio::runtime::Runtime::new().expect("Tokio");
    let res: anyhow::Result<()> = rt.block_on(async {
        let account = |u: &'static str| async move {
            match api::login(u, "admintest1432").await {
                Ok(r) => Ok(r),
                Err(_) => api::register(u, "admintest1432").await,
            }
        };
        api::set_token(Some(account("r2ersteller").await?.token));
        let inst = instances::create("R2 Ersteller", "1.20.1", "fabric", Some("0.20.1".into()))?;
        let path = inst.game_dir().join("mods").join("eigener-r2-mod-1.0.jar");
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, &data)?;
        let sha1 = net::sha1_bytes(&data);
        let pack = api::create_pack(&inst.name, "", &instances::build_manifest(&inst, false)?).await?;
        println!("Erster Upload überträgt: {}", api::upload_file(&pack.id, &sha1, &path).await?);
        println!("Zweiter Upload überträgt: {}", api::upload_file(&pack.id, &sha1, &path).await?);
        let bad = api::upload_file(&pack.id, &"0".repeat(40), &path).await;
        println!("Falsche Prüfsumme abgelehnt: {}", bad.is_err());
        let share = api::create_share(&pack.id, 0, 0).await?;

        api::set_token(Some(account("r2fremder").await?.token));
        println!("Fremder bekommt die Datei: {}", api::download_file(&pack.id, &sha1).await?.is_some());
        api::set_token(Some(account("r2freund").await?.token));
        api::join(&share.code).await?;
        let dir = paths::cache().join("r2test");
        let item = net::DlItem { url: p2p::url(&pack.id, &sha1), path: dir.join("geladen.jar"), sha1: Some(sha1.clone()), size: data.len() as u64 };
        net::download_one(&item).await?;
        println!("Freund lädt ohne Anbieter, Prüfsumme korrekt: {}", net::sha1_file(&item.path)? == sha1);
        std::fs::remove_dir_all(&dir)?;
        instances::delete(&inst.id)?;
        api::set_token(Some(account("r2ersteller").await?.token));
        api::delete_pack(&pack.id).await?;
        api::set_token(Some(account("r2freund").await?.token));
        println!("Nach dem Löschen weg: {}", api::download_file(&pack.id, &sha1).await?.is_none());
        Ok(())
    });
    match res {
        Ok(()) => { println!("OK"); 0 }
        Err(e) => { println!("FEHLER {e:#}"); 1 }
    }
}

/// Entwickler-Test: derselbe Ablauf wie „Spielen“ über den offiziellen Launcher, nur ohne ihn zu öffnen
#[cfg(debug_assertions)]
pub fn play_test() -> i32 {
    std::env::set_var("NNN_DRY_LAUNCH", "1");
    let rt = tokio::runtime::Runtime::new().expect("Tokio");
    let s = crate::settings::load();
    println!("launchMode {}", s.launch_mode);
    for inst in instances::list() {
        println!("Instanz {} ({}, {} {:?})", inst.id, inst.manifest.minecraft, inst.manifest.loader, inst.manifest.loader_version);
        match rt.block_on(official::launch(&inst, s.memory_mb, &s.jvm_args, &s.official_launcher_path, &|p| println!("  {p}"))) {
            Ok(r) => println!("  OK {}", r.profile),
            Err(e) => println!("  FEHLER {e:#}"),
        }
    }
    0
}

/// Entwickler-Test der Anmeldekette: speichert nichts und gibt nur Name oder Fehler aus, nie Tokens.
#[cfg(debug_assertions)]
pub fn ms_test() -> i32 {
    let rt = tokio::runtime::Runtime::new().expect("Tokio");
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let res = rt.block_on(crate::msauth::login(|c| println!("CODE {} {}", c.user_code, c.url), &cancel));
    match res {
        Ok(a) => {
            println!("OK Anmeldung vollständig, Java-Name: {}", a.name);
            0
        }
        Err(e) => {
            println!("FEHLER {e:#}");
            1
        }
    }
}

/// Entwickler-Test des eigenen Spielstarts: legt eine Testinstanz an, startet sie offline und
/// beendet das Spiel nach `secs` Sekunden wieder.
#[cfg(debug_assertions)]
pub fn game_test(loader: &str, mc: &str, version: &str, secs: u64) -> i32 {
    use std::sync::Arc;
    let rt = tokio::runtime::Runtime::new().expect("Tokio");
    let res: anyhow::Result<()> = rt.block_on(async {
        let lv = (!version.is_empty() && version != "-").then(|| version.to_string());
        let inst = instances::create(&format!("Spielstart-Test {loader}"), mc, loader, lv)?;
        let acc = crate::session::Account { id: "00000000000000000000000000000001".into(), name: "NNNTest".into(), kind: "legacy".into(), ..Default::default() };
        let res = async {
            let progress: crate::game::Progress = Arc::new(|s| println!("{s}"));
            let mut run = crate::game::launch(&inst, &acc, 3072, "", progress).await?;
            println!("Gestartet, Log: {}", run.log.display());
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
            run.kill().await;
            let log = std::fs::read_to_string(&run.log)?;
            std::fs::copy(&run.log, paths::logs().join(format!("gametest-{loader}.log")))?;
            println!("Log: {} Zeilen", log.lines().count());
            anyhow::Ok(())
        }
        .await;
        instances::delete(&inst.id)?;
        res
    });
    match res {
        Ok(()) => 0,
        Err(e) => {
            println!("FEHLER: {e:#}");
            1
        }
    }
}

/// Entwickler-Test gegen einen lokalen Server (NNN_SERVER): zwei Konten, Teilen, Beitreten,
/// installierte Version melden, Update erzwingen und automatisch anwenden.
#[cfg(debug_assertions)]
pub fn sync_test() -> i32 {
    use crate::api;
    let rt = tokio::runtime::Runtime::new().expect("Tokio");
    let mut log = std::fs::File::create(paths::logs().join("synctest.log")).expect("Log");
    let mut w = |s: String| {
        let _ = writeln!(log, "{s}");
    };
    let res: anyhow::Result<()> = rt.block_on(async {
        let login = |u: &'static str| async move {
            let r = match api::login(u, "admintest1432").await {
                Ok(r) => r,
                Err(_) => api::register(u, "admintest1432").await?,
            };
            api::set_token(Some(r.token));
            anyhow::Ok(r.user)
        };
        // Ersteller: Instanz mit Texturepack teilen
        let admin = login("admin").await?;
        let a = instances::create("Sync Ersteller", "1.20.1", "fabric", Some("0.20.1".into()))?;
        let (hits, _) = mods::search(Kind::ResourcePack, "modrinth", "faithful", Some(&a), 0).await?;
        let pack_entry = mods::resolve_with_deps(Kind::ResourcePack, &hits[0].source, &hits[0].id, &hits[0].title, None, &a).await?.remove(0);
        let a = instances::add_item(&a.id, Kind::ResourcePack, pack_entry).await?;
        let pack = api::create_pack(&a.name, "", &instances::build_manifest(&a, false)?).await?;
        instances::link_pack(&a.id, &pack.id, 1, "owner")?;
        let share = api::create_share(&pack.id, 0, 0).await?;
        w(format!("{} teilt Pack {} (Code {})", admin.username, pack.id, share.code));

        // Freund: beitreten, installieren, Version melden
        let friend = login("freund1").await?;
        api::join(&share.code).await?;
        let v1 = api::latest(&pack.id).await?;
        let b = instances::create("Sync Freund", &v1.manifest.minecraft, &v1.manifest.loader, v1.manifest.loader_version.clone())?;
        instances::link_pack(&b.id, &pack.id, 0, "member")?;
        let b = instances::apply_manifest(&b.id, v1.manifest, v1.version, Some("member".into()), |_, _| {}).await?;
        api::report_installed(&pack.id, b.pack_version).await?;
        let rp: Vec<String> = std::fs::read_dir(b.game_dir().join("resourcepacks"))?.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        w(format!("{} hat v{} installiert, resourcepacks/: {}", friend.username, b.pack_version, rp.join(", ")));

        // Ersteller: Texturepack entfernen, Shader dazu, v2 veröffentlichen und erzwingen
        login("admin").await?;
        let mut a = instances::get(&a.id)?;
        let file = a.manifest.resourcepacks[0].file_name.clone();
        a = instances::remove_item(&a.id, Kind::ResourcePack, &file)?;
        let (hits, _) = mods::search(Kind::Shader, "modrinth", "complementary", Some(&a), 0).await?;
        let shader = mods::resolve_with_deps(Kind::Shader, &hits[0].source, &hits[0].id, &hits[0].title, None, &a).await?.remove(0);
        a = instances::add_item(&a.id, Kind::Shader, shader).await?;
        let v2 = api::push_version(&pack.id, 1, &instances::build_manifest(&a, false)?, "Shader statt Texturepack").await?;
        api::report_installed(&pack.id, v2).await?;
        let forced = api::force_update(&pack.id).await?;
        let members: Vec<String> = forced.members.iter().map(|m| format!("{}:v{}", m.username, m.installed_version)).collect();
        w(format!("v{v2} veröffentlicht, erzwungen: force={} von {} · Mitglieder {}", forced.force_version, forced.force_by, members.join(", ")));

        // Freund: Zwangs-Update erkennen und anwenden (gleiche Bedingung wie im Launcher)
        login("freund1").await?;
        let p = api::packs().await?.into_iter().find(|p| p.id == pack.id).ok_or_else(|| anyhow::anyhow!("Pack fehlt"))?;
        let b = instances::get(&b.id)?;
        let must = p.force_version > b.pack_version;
        w(format!("Freund sieht force={} bei installierter v{} → automatisch aktualisieren: {must}", p.force_version, b.pack_version));
        if must {
            let v = api::latest(&pack.id).await?;
            let b = instances::apply_manifest(&b.id, v.manifest, v.version, None, |_, _| {}).await?;
            api::report_installed(&pack.id, b.pack_version).await?;
            let rp: Vec<String> = std::fs::read_dir(b.game_dir().join("resourcepacks"))?.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
            let sp: Vec<String> = std::fs::read_dir(b.game_dir().join("shaderpacks"))?.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
            w(format!("Nach Sync: v{} · resourcepacks/: [{}] · shaderpacks/: [{}]", b.pack_version, rp.join(", "), sp.join(", ")));
        }
        login("admin").await?;
        let fin = api::pack(&pack.id).await?;
        w(format!("Mitgliederliste beim Ersteller: {}", fin.members.iter().map(|m| format!("{} v{}", m.username, m.installed_version)).collect::<Vec<_>>().join(", ")));

        api::delete_pack(&pack.id).await?;
        instances::delete(&a.id)?;
        instances::delete(&b.id)?;
        w("Aufgeräumt".into());
        Ok(())
    });
    match res {
        Ok(()) => { w("OK".into()); 0 }
        Err(e) => { w(format!("FEHLER: {e:#}")); 1 }
    }
}

/// Entwickler-Test P2P, Teil 1 (Ersteller): Pack mit eigener Datei teilen und anbieten.
/// Schreibt Pack-ID und Code nach `<out>` und bietet die Datei `secs` Sekunden lang an.
#[cfg(debug_assertions)]
pub fn p2p_seed(out: &str, secs: u64) -> i32 {
    use crate::{api, p2p};
    let rt = tokio::runtime::Runtime::new().expect("Tokio");
    let res: anyhow::Result<()> = rt.block_on(async {
        let r = match api::login("admin", "admintest1432").await { Ok(r) => r, Err(_) => api::register("admin", "admintest1432").await? };
        api::set_token(Some(r.token));
        let inst = instances::create("P2P Ersteller", "1.20.1", "fabric", Some("0.20.1".into()))?;
        // Eine "selbst gebaute" Mod (gibt es nirgends online), 3 MB groß
        let path = inst.game_dir().join("mods").join("eigener-mod-1.0.jar");
        let mut data = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut data));
            let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            z.start_file("fabric.mod.json", o)?;
            z.write_all(br#"{"schemaVersion":1,"id":"eigen","name":"Eigener Mod","version":"1.0"}"#)?;
            z.start_file("daten.bin", o)?;
            let mut x: u32 = 12345;
            for _ in 0..(3 * 1024 * 1024) {
                x = x.wrapping_mul(1103515245).wrapping_add(12345);
                z.write_all(&[(x >> 16) as u8])?;
            }
            z.finish()?;
        }
        std::fs::write(&path, &data)?;
        let sha1 = crate::net::sha1_bytes(&data);
        let pack = api::create_pack(&inst.name, "", &instances::build_manifest(&inst, false)?).await?;
        let entry = instances::ModEntry {
            source: "p2p".into(), project_id: sha1.clone(), version_id: sha1.clone(), name: "Eigener Mod".into(),
            version_name: Some("1.0".into()), file_name: "eigener-mod-1.0.jar".into(), url: p2p::url(&pack.id, &sha1),
            sha1: Some(sha1.clone()), size: data.len() as u64, ..Default::default()
        };
        let inst = instances::adopt(&inst.id, vec![(Kind::Mod, entry)], &[])?;
        let v = api::push_version(&pack.id, 1, &instances::build_manifest(&inst, false)?, "eigene Datei").await?;
        instances::link_pack(&inst.id, &pack.id, v, "owner")?;
        let share = api::create_share(&pack.id, 0, 0).await?;
        let id = p2p::node_id().await?;
        let n = p2p::announce().await?;
        std::fs::write(out, format!("{} {} {} {}", pack.id, share.code, sha1, inst.id))?;
        println!("Ersteller {id} bietet {n} Datei(en) an, Pack {} Code {}", pack.id, share.code);
        let start = std::time::Instant::now();
        while start.elapsed().as_secs() < secs {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            let _ = p2p::announce().await;
        }
        Ok(())
    });
    match res { Ok(()) => 0, Err(e) => { println!("FEHLER Ersteller: {e:#}"); 1 } }
}

/// Entwickler-Test P2P, Teil 2 (Freund): beitreten und die Datei direkt vom Ersteller laden.
#[cfg(debug_assertions)]
pub fn p2p_fetch(info: &str) -> i32 {
    use crate::{api, p2p};
    let rt = tokio::runtime::Runtime::new().expect("Tokio");
    let res: anyhow::Result<()> = rt.block_on(async {
        let s = std::fs::read_to_string(info)?;
        let parts: Vec<&str> = s.split_whitespace().collect();
        let (pack_id, code, sha1) = (parts[0], parts[1], parts[2]);
        let r = match api::login("freund1", "admintest1432").await { Ok(r) => r, Err(_) => api::register("freund1", "admintest1432").await? };
        api::set_token(Some(r.token));
        api::join(code).await?;
        // Eigene iroh-ID beim Server bekannt machen, damit der Ersteller uns als Mitglied erkennt
        let me = p2p::node_id().await?;
        p2p::announce().await?;
        println!("Freund {me} ist beigetreten, Anbieter laut Server: {:?}", api::p2p_peers(pack_id, sha1).await?);
        let v = api::latest(pack_id).await?;
        let b = instances::create("P2P Freund", &v.manifest.minecraft, &v.manifest.loader, v.manifest.loader_version.clone())?;
        instances::link_pack(&b.id, pack_id, 0, "member")?;
        let t0 = std::time::Instant::now();
        let b = instances::apply_manifest(&b.id, v.manifest, v.version, Some("member".into()), |_, _| {}).await?;
        let path = b.game_dir().join("mods").join("eigener-mod-1.0.jar");
        let ok = path.exists() && crate::net::sha1_file(&path)? == sha1;
        println!(
            "Übertragung: Datei da und Prüfsumme korrekt = {ok}, {:.1} MB in {:.1} s",
            std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) as f64 / 1048576.0,
            t0.elapsed().as_secs_f64()
        );
        // Ein Fremder ohne Mitgliedschaft darf nichts bekommen
        let outsider = match api::login("fremder", "admintest1432").await { Ok(r) => r, Err(_) => api::register("fremder", "admintest1432").await? };
        api::set_token(Some(outsider.token));
        let denied = api::p2p_peers(pack_id, sha1).await.is_err();
        println!("Fremder ohne Mitgliedschaft bekommt keine Anbieterliste = {denied}");
        instances::delete(&b.id)?;
        if !ok { anyhow::bail!("Datei fehlt nach dem Sync"); }
        Ok(())
    });
    match res { Ok(()) => 0, Err(e) => { println!("FEHLER Freund: {e:#}"); 1 } }
}
