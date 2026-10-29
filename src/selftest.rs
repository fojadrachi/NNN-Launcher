//! Selbsttest ohne Oberfläche: `nnn-launcher.exe --selftest` – Ergebnis in logs\selftest.log.
//! Legt eine Fabric-Instanz an, installiert Sodium samt Abhängigkeiten, bereitet den Start
//! über den offiziellen Launcher vor (ohne ihn zu öffnen) und räumt danach wieder auf.

use crate::{instances, loaders, minecraft, mods, official, paths};
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
        let (hits, total) = mods::search("modrinth", "sodium", Some(&inst), 0).await?;
        w(format!("Suche 'sodium': {total} Treffer, erster: {}", hits.first().map(|h| h.title.clone()).unwrap_or_default()));
        let hit = hits.first().ok_or_else(|| anyhow::anyhow!("kein Treffer"))?;
        let list = mods::resolve_with_deps(&hit.source, &hit.id, &hit.title, hit.icon.clone(), &inst).await?;
        w(format!("Aufgelöst: {}", list.iter().map(|m| format!("{} ({})", m.name, m.file_name)).collect::<Vec<_>>().join(", ")));
        let mut last = inst.clone();
        for m in list {
            last = instances::add_mod(&inst.id, m).await?;
        }
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
