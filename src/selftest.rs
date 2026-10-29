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
