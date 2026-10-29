//! Steuerung: hält den Zustand, führt Aktionen im Hintergrund aus und spiegelt alles in die Oberfläche.

use crate::instances::{self, Instance, Kind};
use crate::settings::{self, Event, NotifStore, Outgoing, Settings};
use crate::session::{self, User};
use crate::{api, loaders, minecraft, mods, official, p2p, scan, updater, AppState, AppWindow};
use crate::{HitItem, InstanceItem, MemberItem, ModItem, NotifItem, PackItem, ToastItem, UserItem, VersionItem};
use anyhow::Result;
use slint::{ComponentHandle, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

type Pixels = SharedPixelBuffer<Rgba8Pixel>;

#[derive(Clone, Default)]
pub struct Data {
    instances: Vec<Instance>,
    packs: Vec<api::Pack>,
    friends: api::Friends,
    user: Option<User>,
    server_online: bool,
    current: Option<String>,
    current_pack: Option<api::Pack>,
    current_versions: Vec<api::VersionInfo>,
    busy: HashMap<String, (String, f32)>,
    hits: Vec<mods::Hit>,
    hits_total: usize,
    hits_loading: bool,
    hits_error: String,
    hits_kind: Kind,
    hit_busy: HashSet<usize>,
    search_seq: u64,
    mc_versions: Vec<String>,
    loader_versions: Vec<loaders::LoaderVersion>,
    loader_loading: bool,
    loader_seq: u64,
    images: HashMap<String, Pixels>,
    images_pending: HashSet<String>,
    notif: NotifStore,
    last_unseen: usize,
    bell_ring: bool,
    settings: Settings,
    update: Option<updater::UpdateInfo>,
    update_state: String,
    update_busy: bool,
    toasts: Vec<(i32, String, String)>,
    toast_seq: i32,
    modal_busy: bool,
    share: Option<(String, String)>,
    join_preview: String,
    join_ready: bool,
    /// Bereits an den Server gemeldete Versionen (Pack-ID → Version)
    reported: HashMap<String, i64>,
    /// Zwangs-Updates, die in dieser Sitzung schon versucht wurden
    forced_tried: HashSet<String>,
    /// Dateien im Instanzordner, die nicht im Pack sind (nach Ordner-Scan)
    scan_found: HashMap<String, Vec<scan::Found>>,
    scanning: HashSet<String>,
    /// Gerade laufende P2P-Nachlade-Vorgänge (Instanz-IDs)
    p2p_fetching: HashSet<String>,
}

pub struct Ctrl {
    pub data: Mutex<Data>,
    pub ui: slint::Weak<AppWindow>,
    pub rt: tokio::runtime::Handle,
    refresh_pending: Mutex<bool>,
}

pub type C = Arc<Ctrl>;

impl Ctrl {
    pub fn new(ui: slint::Weak<AppWindow>, rt: tokio::runtime::Handle) -> C {
        let mut d = Data { settings: settings::load(), server_online: true, ..Default::default() };
        d.instances = instances::list();
        if let Some((token, user)) = session::load() {
            api::set_token(Some(token));
            d.notif = settings::load_notifs(user.id);
            d.user = Some(user);
        }
        Arc::new(Ctrl { data: Mutex::new(d), ui, rt, refresh_pending: Mutex::new(false) })
    }

    fn with<R>(&self, f: impl FnOnce(&mut Data) -> R) -> R { f(&mut self.data.lock().unwrap()) }

    pub fn spawn<F>(self: &C, f: impl FnOnce(C) -> F + Send + 'static)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let c = self.clone();
        self.rt.spawn(async move { f(c).await });
    }

    /// UI neu befüllen (gebündelt, höchstens ~30× pro Sekunde)
    pub fn refresh(self: &C) {
        {
            let mut p = self.refresh_pending.lock().unwrap();
            if *p {
                return;
            }
            *p = true;
        }
        let c = self.clone();
        self.rt.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            *c.refresh_pending.lock().unwrap() = false;
            let d = c.data.lock().unwrap().clone();
            let c2 = c.clone();
            let _ = c.ui.upgrade_in_event_loop(move |ui| apply(&c2, &ui, &d));
        });
    }

    pub fn toast(self: &C, text: impl Into<String>, kind: &str) {
        let id = self.with(|d| {
            d.toast_seq += 1;
            d.toasts.push((d.toast_seq, text.into(), kind.to_string()));
            if d.toasts.len() > 4 {
                d.toasts.remove(0);
            }
            d.toast_seq
        });
        self.refresh();
        let ms = if kind == "error" { 7000 } else { 4500 };
        self.spawn(move |c| async move {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            c.with(|d| d.toasts.retain(|t| t.0 != id));
            c.refresh();
        });
    }

    pub fn fail(self: &C, e: anyhow::Error) {
        if e.downcast_ref::<api::Unauthorized>().is_some() {
            self.set_session(None, None);
        }
        self.toast(format!("{e:#}").replace(": ", " – "), "error");
    }

    fn set_session(self: &C, token: Option<String>, user: Option<User>) {
        session::save(token.as_deref(), user.as_ref());
        api::set_token(token);
        self.with(|d| {
            d.notif = user.as_ref().map(|u| settings::load_notifs(u.id)).unwrap_or_default();
            d.user = user;
            if d.user.is_none() {
                d.packs.clear();
                d.friends = Default::default();
            }
        });
        self.refresh();
    }

    fn set_busy(self: &C, key: &str, status: Option<(&str, f32)>) {
        self.with(|d| match status {
            Some((s, p)) => {
                d.busy.insert(key.to_string(), (s.to_string(), p));
            }
            None => {
                d.busy.remove(key);
            }
        });
        self.refresh();
    }

    fn progress_fn(self: &C, key: &str) -> impl Fn(String) + Send + Sync + 'static {
        let c = self.clone();
        let key = key.to_string();
        move |s: String| c.set_busy(&key, Some((&s, -1.0)))
    }

    fn reload_instances(self: &C) {
        let list = instances::list();
        self.with(|d| d.instances = list);
        self.refresh();
    }

    pub fn toggle_item(self: &C, id: String, kind: Kind, file: String, enabled: bool) {
        match instances::set_enabled(&id, kind, &file, enabled) {
            Ok(()) => self.toast(format!("{} {}", file, if enabled { "eingeschaltet" } else { "ausgeschaltet" }), ""),
            Err(e) => self.fail(e),
        }
        self.refresh();
    }

    pub fn current_id(&self) -> Option<String> { self.with(|d| d.current.clone()) }

    pub fn dismiss_toast(self: &C, id: i32) {
        self.with(|d| d.toasts.retain(|t| t.0 != id));
        self.refresh();
    }

    pub fn confirmed_delete(self: &C, id: String) {
        match instances::delete(&id) {
            Ok(()) => {
                self.with(|d| {
                    d.instances.retain(|i| i.id != id);
                    d.current = None;
                });
                let _ = self.ui.upgrade_in_event_loop(|ui| ui.global::<AppState>().set_page("library".into()));
                self.toast("Instanz gelöscht", "");
            }
            Err(e) => self.fail(e),
        }
    }

    pub fn upsert(self: &C, i: Instance) {
        self.with(|d| match d.instances.iter_mut().find(|x| x.id == i.id) {
            Some(x) => *x = i,
            None => d.instances.insert(0, i),
        });
        self.refresh();
    }

    fn inst(&self, id: &str) -> Option<Instance> { self.with(|d| d.instances.iter().find(|i| i.id == id).cloned()) }

    fn want_image(self: &C, url: &str) {
        if url.is_empty() {
            return;
        }
        let start = self.with(|d| {
            if d.images.contains_key(url) || d.images_pending.contains(url) {
                false
            } else {
                d.images_pending.insert(url.to_string());
                true
            }
        });
        if !start {
            return;
        }
        let url = url.to_string();
        self.spawn(move |c| async move {
            if let Some(px) = load_image(&url).await {
                c.with(|d| {
                    d.images.insert(url.clone(), px);
                });
                c.refresh();
            }
        });
    }

    // ---------- Laden ----------

    pub async fn refresh_server(self: &C) {
        if !api::logged_in() {
            return;
        }
        let (packs, friends) = tokio::join!(api::packs(), api::friends());
        let online = packs.is_ok() || friends.is_ok();
        if let Err(e) = &packs {
            if e.downcast_ref::<api::Unauthorized>().is_some() {
                self.set_session(None, None);
                self.toast("Sitzung abgelaufen – bitte neu anmelden", "warn");
                return;
            }
        }
        let mut new_toasts = Vec::new();
        self.with(|d| {
            d.server_online = online;
            if let Ok(p) = packs {
                d.packs = p;
            }
            if let Ok(f) = friends {
                // Wer aus "Gesendet" jetzt Freund ist, hat angenommen
                let ids: HashSet<i64> = f.friends.iter().map(|x| x.id).collect();
                let before = std::mem::take(&mut d.notif.outgoing);
                for o in before.iter().filter(|o| ids.contains(&o.id)) {
                    let key = format!("fa:{}", o.id);
                    if !d.notif.events.iter().any(|e| e.key == key) {
                        let title = format!("{} hat deine Freundschaftsanfrage angenommen", o.username);
                        new_toasts.push(title.clone());
                        d.notif.events.insert(0, Event { key, kind: "friend-accepted".into(), title, user: o.username.clone(), time: instances::now() });
                    }
                }
                d.notif.outgoing = f.outgoing.iter().map(|o| Outgoing { id: o.id, username: o.username.clone() }).collect();
                d.friends = f;
            }
            if let Some(u) = &d.user {
                settings::save_notifs(u.id, &d.notif);
            }
        });
        for t in new_toasts {
            self.toast(t, "");
        }
        // Installierte Versionen melden und vom Ersteller erzwungene Updates ausführen
        let (reports, forced) = self.with(|d| {
            let mut reports = Vec::new();
            let mut forced = Vec::new();
            for i in &d.instances {
                let Some(p) = i.pack_id.as_ref().and_then(|pid| d.packs.iter().find(|x| &x.id == pid)) else { continue };
                if d.reported.get(&p.id) != Some(&i.pack_version) && i.pack_version > 0 {
                    reports.push((p.id.clone(), i.pack_version));
                }
                let key = format!("{}:{}", p.id, p.force_version);
                if p.force_version > i.pack_version && !d.busy.contains_key(&i.id) && !d.forced_tried.contains(&key) {
                    d.forced_tried.insert(key);
                    forced.push((i.id.clone(), p.force_by.clone()));
                }
            }
            (reports, forced)
        });
        for (pid, v) in reports {
            self.report(pid, v);
        }
        for (id, by) in forced {
            self.sync_with(id, Some(by));
        }
        self.refresh();
        self.check_bell();
    }

    fn report(self: &C, pack_id: String, version: i64) {
        self.spawn(move |c| async move {
            if api::report_installed(&pack_id, version).await.is_ok() {
                c.with(|d| d.reported.insert(pack_id, version));
            }
        });
    }

    fn check_bell(self: &C) {
        let ring = self.with(|d| {
            let unseen = build_notifs(d).iter().filter(|n| n.unseen).count();
            let ring = unseen > d.last_unseen;
            d.last_unseen = unseen;
            if ring {
                d.bell_ring = true;
            }
            ring
        });
        if ring {
            self.refresh();
            self.spawn(|c| async move {
                tokio::time::sleep(std::time::Duration::from_millis(950)).await;
                c.with(|d| d.bell_ring = false);
                c.refresh();
            });
        }
    }

    pub async fn load_current_pack(self: &C) {
        let Some(id) = self.with(|d| d.current.clone()) else { return };
        let Some(inst) = self.inst(&id) else { return };
        let Some(pid) = inst.pack_id.clone() else {
            self.with(|d| {
                d.current_pack = None;
                d.current_versions.clear();
            });
            return;
        };
        if !api::logged_in() {
            return;
        }
        let (p, v) = tokio::join!(api::pack(&pid), api::versions(&pid));
        if let Ok(p) = &p {
            for m in &p.members {
                self.want_image(&avatar_url(&m.username));
            }
        }
        self.with(|d| {
            if d.current.as_deref() == Some(id.as_str()) {
                d.current_pack = p.ok();
                d.current_versions = v.unwrap_or_default();
            }
        });
        self.refresh();
    }

    pub async fn ensure_mc_versions(self: &C) {
        if self.with(|d| !d.mc_versions.is_empty()) {
            return;
        }
        match minecraft::release_versions().await {
            Ok(v) => self.with(|d| d.mc_versions = v),
            Err(e) => self.fail(e),
        }
        self.refresh();
    }

    pub async fn load_loader_versions(self: &C, mc: String, loader_idx: i32) {
        let loader = loaders::LOADERS.get(loader_idx.max(0) as usize).copied().unwrap_or("vanilla");
        // Nur das Ergebnis der letzten Anfrage zählt (schnelles Umschalten, Dialog vs. Detailseite)
        let seq = self.with(|d| {
            d.loader_seq += 1;
            d.loader_loading = true;
            d.loader_versions.clear();
            d.loader_seq
        });
        self.refresh();
        let res = if loader == "vanilla" { Ok(vec![]) } else { loaders::list(loader, &mc).await };
        let current = self.with(|d| {
            if d.loader_seq != seq {
                return false;
            }
            d.loader_loading = false;
            d.loader_versions = res.as_ref().cloned().unwrap_or_default();
            true
        });
        if !current {
            return;
        }
        if let Err(e) = res {
            self.fail(e);
        }
        self.refresh();
    }

    pub async fn check_update(self: &C, manual: bool) {
        self.with(|d| d.update_busy = manual);
        self.refresh();
        match updater::check().await {
            Ok(u) => {
                let new = self.with(|d| {
                    let new = u.available && d.update.as_ref().map(|x| x.latest != u.latest).unwrap_or(true);
                    d.update_state = if u.available {
                        format!("Neue Version {} verfügbar", u.latest)
                    } else {
                        format!("Aktuell · zuletzt geprüft {}", chrono::Local::now().format("%H:%M"))
                    };
                    d.update = Some(u.clone());
                    new
                });
                if new {
                    self.toast(format!("NNN Launcher {} ist verfügbar – siehe Glocke", u.latest), "");
                } else if manual && !u.available {
                    self.toast("Du hast die neueste Version", "");
                }
            }
            Err(e) => {
                self.with(|d| d.update_state = "Update-Prüfung fehlgeschlagen".into());
                if manual {
                    self.fail(e);
                }
            }
        }
        self.with(|d| d.update_busy = false);
        self.refresh();
        self.check_bell();
    }

    // ---------- Aktionen ----------

    pub fn open_instance(self: &C, id: &str) {
        let Some(inst) = self.inst(id) else { return };
        self.with(|d| {
            if d.current.as_deref() != Some(id) {
                d.current_pack = None;
                d.current_versions.clear();
            }
            d.current = Some(id.to_string());
        });
        for k in Kind::ALL {
            for m in inst.manifest.list(k) {
                if let Some(u) = &m.icon_url {
                    self.want_image(u);
                }
            }
        }
        self.refresh();
        let mc = inst.manifest.minecraft.clone();
        let li = loaders::LOADERS.iter().position(|l| *l == inst.manifest.loader).unwrap_or(0) as i32;
        self.scan_instance(id.to_string(), false);
        self.spawn(move |c| async move {
            c.ensure_mc_versions().await;
            c.load_loader_versions(mc, li).await;
            c.load_current_pack().await;
        });
    }

    /// Ordner der Instanz scannen. Erkannte Dateien werden übernommen (wenn man das Pack bearbeiten darf),
    /// von Hand gelöschte aus der Liste entfernt.
    pub fn scan_instance(self: &C, id: String, manual: bool) {
        if !self.with(|d| d.scanning.insert(id.clone())) {
            return;
        }
        if manual {
            self.refresh();
        }
        self.spawn(move |c| async move {
            let res: Result<(usize, usize)> = async {
                let inst = instances::get(&id)?;
                let r = scan::scan(&inst).await?;
                let can_edit = inst.can_edit();
                let mut adopt = Vec::new();
                let mut rest = Vec::new();
                for mut f in r.found {
                    // Icons aus der Datei für die Anzeige vorbereiten
                    if let Some(png) = f.icon_png.take() {
                        let key = format!("local:{}/{}", id, f.file_name);
                        if let Some(px) = tokio::task::spawn_blocking(move || decode(&png)).await.ok().flatten() {
                            c.with(|d| d.images.insert(key, px));
                        }
                    }
                    if let Some(u) = f.entry.as_ref().and_then(|e| e.icon_url.clone()) {
                        c.want_image(&u);
                    }
                    match (&f.entry, f.kind) {
                        (Some(e), Some(k)) if can_edit => adopt.push((k, e.clone())),
                        _ => rest.push(f),
                    }
                }
                let missing = if can_edit { r.missing } else { vec![] };
                let (added, removed) = (adopt.len(), missing.len());
                if added > 0 || removed > 0 {
                    let i = instances::adopt(&id, adopt, &missing)?;
                    c.upsert(i);
                }
                c.with(|d| {
                    d.scan_found.insert(id.clone(), rest);
                });
                Ok((added, removed))
            }
            .await;
            c.with(|d| d.scanning.remove(&id));
            match res {
                Ok((a, r)) if a > 0 || r > 0 => {
                    let mut parts = Vec::new();
                    if a > 0 {
                        parts.push(format!("{a} neue Datei(en) erkannt und übernommen"));
                    }
                    if r > 0 {
                        parts.push(format!("{r} gelöschte entfernt"));
                    }
                    c.toast(format!("Ordner gescannt: {}", parts.join(", ")), "");
                }
                Ok(_) if manual => c.toast("Ordner gescannt – alles aktuell", ""),
                Ok(_) => {}
                Err(e) if manual => c.fail(e),
                Err(_) => {}
            }
            c.refresh();
        });
    }

    pub fn play(self: &C, id: String) {
        let Some(inst) = self.inst(&id) else { return };
        if self.with(|d| d.busy.contains_key(&id)) {
            return;
        }
        self.set_busy(&id, Some(("Vorbereiten", -1.0)));
        self.spawn(move |c| async move {
            let s = c.with(|d| d.settings.clone());
            let progress = c.progress_fn(&id);
            match official::launch(&inst, s.memory_mb, &s.jvm_args, &s.official_launcher_path, &progress).await {
                Ok(r) => {
                    c.toast(format!("Minecraft Launcher geöffnet – Profil „{}“ ist ausgewählt. Dort auf „Spielen“ klicken.", r.profile), "");
                    if r.already_running {
                        c.toast("Der Minecraft Launcher war schon offen. Fehlt das Profil, Launcher schließen und hier nochmal auf „Spielen“ klicken.", "warn");
                    }
                    if s.close_on_launch {
                        let _ = c.ui.upgrade_in_event_loop(|ui| ui.window().set_minimized(true));
                    }
                    c.reload_instances();
                }
                Err(e) => c.fail(e),
            }
            c.set_busy(&id, None);
        });
    }

    pub fn sync(self: &C, id: String) { self.sync_with(id, None) }

    /// `forced_by`: Update wurde vom Ersteller erzwungen und läuft automatisch
    pub fn sync_with(self: &C, id: String, forced_by: Option<String>) {
        let Some(inst) = self.inst(&id) else { return };
        let Some(pid) = inst.pack_id.clone() else { return };
        let role = self.with(|d| d.packs.iter().find(|p| p.id == pid).map(|p| p.role.clone()));
        self.set_busy(&id, Some(("Update laden", -1.0)));
        self.spawn(move |c| async move {
            let res: Result<(i64, String)> = async {
                let v = api::latest(&pid).await?;
                let cc = c.clone();
                let key = id.clone();
                let i = instances::apply_manifest(&id, v.manifest, v.version, role, move |done, total| {
                    cc.set_busy(&key, Some(("Mods synchronisieren", done as f32 / total.max(1) as f32)));
                })
                .await?;
                c.upsert(i);
                Ok((v.version, v.changelog))
            }
            .await;
            match res {
                Ok((v, log)) => {
                    c.report(pid.clone(), v);
                    match &forced_by {
                        Some(by) => {
                            let title = format!("„{}“ wurde automatisch auf Version {v} aktualisiert", inst.name);
                            let text = if by.is_empty() { String::new() } else { format!(" (erzwungen von {by})") };
                            c.with(|d| {
                                d.notif.events.insert(0, Event { key: format!("forced:{pid}:{v}"), kind: "update".into(), title: format!("{title}{text}"), user: String::new(), time: instances::now() });
                                if let Some(u) = &d.user {
                                    settings::save_notifs(u.id, &d.notif);
                                }
                            });
                            c.toast(format!("{title}{text}"), "");
                        }
                        None => c.toast(format!("Auf Version {v} aktualisiert{}", if log.is_empty() { String::new() } else { format!(": {log}") }), ""),
                    }
                }
                Err(e) => c.fail(e),
            }
            c.set_busy(&id, None);
            c.load_current_pack().await;
            c.check_bell();
        });
    }

    pub fn install_pack(self: &C, pack_id: String, navigate: bool) {
        let key = format!("pack:{pack_id}");
        if self.with(|d| d.busy.contains_key(&key)) {
            return;
        }
        self.set_busy(&key, Some(("Pack laden", -1.0)));
        self.spawn(move |c| async move {
            let res: Result<Instance> = async {
                let pack = match c.with(|d| d.packs.iter().find(|p| p.id == pack_id).cloned()) {
                    Some(p) => p,
                    None => api::pack(&pack_id).await?,
                };
                let v = api::latest(&pack_id).await?;
                let existing = instances::list().into_iter().find(|i| i.pack_id.as_deref() == Some(pack_id.as_str()));
                let inst = match existing {
                    Some(i) => i,
                    None => {
                        let i = instances::create(&pack.name, &v.manifest.minecraft, &v.manifest.loader, v.manifest.loader_version.clone())?;
                        instances::link_pack(&i.id, &pack_id, 0, if pack.role.is_empty() { "member" } else { &pack.role })?
                    }
                };
                let cc = c.clone();
                let k = key.clone();
                let role = Some(if pack.role.is_empty() { "member".to_string() } else { pack.role.clone() });
                let i = instances::apply_manifest(&inst.id, v.manifest, v.version, role, move |done, total| {
                    cc.set_busy(&k, Some(("Mods herunterladen", done as f32 / total.max(1) as f32)));
                })
                .await?;
                Ok(i)
            }
            .await;
            c.set_busy(&key, None);
            match res {
                Ok(i) => {
                    c.toast(format!("„{}“ installiert", i.name), "");
                    c.report(pack_id.clone(), i.pack_version);
                    let id = i.id.clone();
                    c.upsert(i);
                    if navigate {
                        c.open_instance(&id);
                        let _ = c.ui.upgrade_in_event_loop(|ui| {
                            let s = ui.global::<AppState>();
                            s.set_tab("mods".into());
                            s.set_page("instance".into());
                        });
                    }
                    c.check_bell();
                }
                Err(e) => c.fail(e),
            }
        });
    }

    pub fn search(self: &C, kind: Kind, source: i32, query: String, target: Option<String>, append: bool) {
        let seq = self.with(|d| {
            d.search_seq += 1;
            d.hits_loading = true;
            d.hits_error.clear();
            if !append || d.hits_kind != kind {
                d.hits.clear();
                d.hit_busy.clear();
                d.hits_total = 0;
            }
            d.hits_kind = kind;
            d.search_seq
        });
        self.refresh();
        let src = if source == 1 { "curseforge" } else { "modrinth" };
        self.spawn(move |c| async move {
            if src == "curseforge" && !api::logged_in() {
                c.with(|d| {
                    d.hits_loading = false;
                    d.hits_error = "CurseForge läuft über den NNN-Server – bitte erst unter „Freunde“ anmelden.".into();
                });
                c.refresh();
                return;
            }
            let inst = target.as_deref().and_then(|t| c.inst(t));
            if kind == Kind::Mod && inst.as_ref().is_some_and(|i| i.manifest.loader == "vanilla") {
                c.with(|d| {
                    d.hits_loading = false;
                    d.hits_error = "Diese Instanz ist Vanilla und hat keinen Mod-Loader. Texturepacks gehen trotzdem – oder wähle oben eine andere Instanz.".into();
                });
                c.refresh();
                return;
            }
            let offset = if append { c.with(|d| d.hits.len()) } else { 0 };
            let res = mods::search(kind, src, &query, inst.as_ref(), offset).await;
            let stale = c.with(|d| d.search_seq != seq);
            if stale {
                return;
            }
            match res {
                Ok((hits, total)) => {
                    for h in &hits {
                        if let Some(u) = &h.icon {
                            c.want_image(u);
                        }
                    }
                    c.with(|d| {
                        d.hits.extend(hits);
                        d.hits_total = total;
                        if d.hits.is_empty() {
                            d.hits_error = "Nichts gefunden.".into();
                        }
                    });
                }
                Err(e) => c.with(|d| d.hits_error = format!("{e:#}")),
            }
            c.with(|d| d.hits_loading = false);
            c.refresh();
        });
    }

    pub fn install_hit(self: &C, idx: usize, target: Option<String>) {
        let Some((hit, kind)) = self.with(|d| d.hits.get(idx).cloned().map(|h| (h, d.hits_kind))) else { return };
        let Some(tid) = target else { return };
        self.with(|d| d.hit_busy.insert(idx));
        self.refresh();
        self.spawn(move |c| async move {
            let res: Result<(String, usize, bool)> = async {
                let inst = c.inst(&tid).ok_or_else(|| anyhow::anyhow!("Instanz nicht gefunden"))?;
                let list = mods::resolve_with_deps(kind, &hit.source, &hit.id, &hit.title, hit.icon.clone(), &inst).await?;
                let n = list.len();
                let main = list[0].name.clone();
                let mut last = inst;
                for m in list {
                    // Abhängigkeiten sind immer Mods
                    let k = if m.dependency { Kind::Mod } else { kind };
                    last = instances::add_item(&tid, k, m).await?;
                    c.upsert(last.clone());
                }
                Ok((main, n, last.has_shader_loader()))
            }
            .await;
            c.with(|d| d.hit_busy.remove(&idx));
            match res {
                Ok((name, n, _)) if n > 1 => c.toast(format!("{name} + {} Abhängigkeit(en) installiert", n - 1), ""),
                Ok((name, _, has_loader)) => {
                    c.toast(format!("{} „{name}“ installiert", kind.label()), "");
                    if kind == Kind::Shader && !has_loader {
                        c.toast("Für Shader brauchst du Iris (Fabric/NeoForge) oder Oculus (Forge) – unter „Mods“ suchen und installieren.", "warn");
                    }
                    if kind == Kind::ResourcePack {
                        c.toast("Im Spiel unter Optionen → Ressourcenpakete aktivieren.", "");
                    }
                }
                Err(e) => c.fail(e),
            }
            c.refresh();
        });
    }

    pub fn create_instance(self: &C, name: String, mc: String, loader_idx: i32, lv: String) {
        let loader = loaders::LOADERS.get(loader_idx.max(0) as usize).copied().unwrap_or("vanilla");
        let name = if name.trim().is_empty() { "Neue Instanz".to_string() } else { name };
        match instances::create(&name, &mc, loader, (!lv.is_empty()).then_some(lv)) {
            Ok(i) => {
                let id = i.id.clone();
                self.upsert(i);
                self.open_instance(&id);
                let _ = self.ui.upgrade_in_event_loop(|ui| {
                    let s = ui.global::<AppState>();
                    s.set_modal("".into());
                    s.set_tab("mods".into());
                    s.set_page("instance".into());
                });
            }
            Err(e) => self.fail(e),
        }
    }

    pub fn save_instance(self: &C, id: String, name: String, memory: String, mc: String, loader_idx: i32, lv: String) {
        let res: Result<Instance> = (|| {
            let mut i = instances::get(&id)?;
            if !name.trim().is_empty() {
                i.name = name.trim().to_string();
                i.manifest.name = i.name.clone();
            }
            i.memory_mb = memory.trim().parse::<u32>().ok().filter(|m| *m >= 1024).map(|m| m.min(65536));
            if loader_idx >= 0 && !mc.is_empty() {
                let loader = loaders::LOADERS.get(loader_idx as usize).copied().unwrap_or("vanilla");
                if loader != "vanilla" && lv.is_empty() {
                    anyhow::bail!("Bitte eine Loader-Version wählen");
                }
                i.manifest.minecraft = mc;
                i.manifest.loader = loader.to_string();
                i.manifest.loader_version = (loader != "vanilla").then_some(lv);
            }
            instances::save(&i)?;
            Ok(i)
        })();
        match res {
            Ok(i) => {
                self.upsert(i);
                self.toast("Gespeichert", "");
            }
            Err(e) => self.fail(e),
        }
    }

    /// Nimmt lokale (nicht auf Modrinth bekannte) Dateien als P2P-Einträge ins Pack auf
    async fn share_local(self: &C, id: &str, pack_id: &str) -> Result<(Instance, usize)> {
        let inst = instances::get(id)?;
        let found = self.with(|d| d.scan_found.get(id).cloned().unwrap_or_default());
        let mut add = Vec::new();
        for k in Kind::ALL {
            for f in instances::local_files(&inst, k) {
                let Some(path) = instances::existing_path(id, k, &f) else { continue };
                let p = path.clone();
                let sha1 = tokio::task::spawn_blocking(move || scan::sha1_of(&p)).await??;
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let info = found.iter().find(|x| x.file_name.eq_ignore_ascii_case(&f));
                add.push((
                    k,
                    instances::ModEntry {
                        source: "p2p".into(),
                        project_id: sha1.clone(),
                        version_id: sha1.clone(),
                        name: info.map(|i| i.meta_name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| f.clone()),
                        version_name: info.map(|i| i.meta_version.clone()).filter(|v| !v.is_empty()),
                        file_name: f.clone(),
                        url: p2p::url(pack_id, &sha1),
                        sha1: Some(sha1),
                        size,
                        ..Default::default()
                    },
                ));
            }
        }
        let n = add.len();
        let i = if n > 0 { instances::adopt(id, add, &[])? } else { inst };
        Ok((i, n))
    }

    /// P2P: eigene Dateien beim Server melden und fehlende von Freunden nachladen
    pub async fn p2p_tick(self: &C) {
        if !api::logged_in() {
            return;
        }
        let _ = p2p::announce().await;
        for inst in self.with(|d| d.instances.clone()) {
            let pending = instances::pending_p2p(&inst);
            if pending.is_empty() || !self.with(|d| d.p2p_fetching.insert(inst.id.clone())) {
                continue;
            }
            let c = self.clone();
            self.rt.spawn(async move {
                let mut got = 0;
                for (k, m) in &pending {
                    let path = instances::game_dir(&inst.id).join(k.folder()).join(&m.file_name);
                    let item = crate::net::DlItem { url: m.url.clone(), path, sha1: m.sha1.clone(), size: m.size };
                    if crate::net::download_one(&item).await.is_ok() {
                        got += 1;
                    }
                }
                c.with(|d| d.p2p_fetching.remove(&inst.id));
                if got > 0 {
                    c.toast(format!("„{}“: {got} Datei(en) von Freunden geladen", inst.name), "");
                    let _ = p2p::announce().await;
                }
                c.refresh();
            });
        }
    }

    pub fn publish(self: &C, id: String, name: String, desc: String, configs: bool, local: bool) {
        self.with(|d| d.modal_busy = true);
        self.refresh();
        self.spawn(move |c| async move {
            let res: Result<()> = async {
                let mut inst = instances::get(&id)?;
                if !name.trim().is_empty() && name.trim() != inst.name {
                    inst.name = name.trim().to_string();
                    inst.manifest.name = inst.name.clone();
                    instances::save(&inst)?;
                }
                let manifest = instances::build_manifest(&inst, configs)?;
                let pack = api::create_pack(&inst.name, &desc, &manifest).await?;
                let mut i = instances::link_pack(&id, &pack.id, 1, "owner")?;
                let mut version = 1;
                // Eigene Dateien brauchen die Pack-ID für ihre P2P-Adresse, daher gleich als Version 2
                if local {
                    let (with_local, n) = c.share_local(&id, &pack.id).await?;
                    if n > 0 {
                        let m = instances::build_manifest(&with_local, configs)?;
                        version = api::push_version(&pack.id, 1, &m, &format!("{n} eigene Datei(en) per P2P")).await?;
                        i = instances::link_pack(&id, &pack.id, version, "owner")?;
                    }
                }
                c.report(pack.id.clone(), version);
                let _ = p2p::announce().await;
                c.upsert(i);
                c.refresh_server().await;
                Ok(())
            }
            .await;
            c.with(|d| {
                d.modal_busy = false;
                d.share = None;
            });
            match res {
                Ok(()) => {
                    c.toast("Pack geteilt – jetzt Freunde einladen oder einen Link erstellen", "");
                    let _ = c.ui.upgrade_in_event_loop(|ui| {
                        let s = ui.global::<AppState>();
                        s.set_tab("pack".into());
                        s.set_modal("share".into());
                    });
                    c.load_current_pack().await;
                }
                Err(e) => c.fail(e),
            }
            c.refresh();
        });
    }

    pub fn push(self: &C, id: String, changelog: String, configs: bool, local: bool) {
        self.with(|d| d.modal_busy = true);
        self.refresh();
        self.spawn(move |c| async move {
            let res: Result<i64> = async {
                let mut inst = instances::get(&id)?;
                let pid = inst.pack_id.clone().ok_or_else(|| anyhow::anyhow!("Instanz ist nicht geteilt"))?;
                if local {
                    inst = c.share_local(&id, &pid).await?.0;
                }
                let manifest = instances::build_manifest(&inst, configs)?;
                let v = api::push_version(&pid, inst.pack_version, &manifest, &changelog).await?;
                let i = instances::link_pack(&id, &pid, v, inst.pack_role.as_deref().unwrap_or("owner"))?;
                c.report(pid.clone(), v);
                let _ = p2p::announce().await;
                c.upsert(i);
                c.refresh_server().await;
                Ok(v)
            }
            .await;
            c.with(|d| d.modal_busy = false);
            match res {
                Ok(v) => {
                    c.toast(format!("Version {v} veröffentlicht – deine Freunde bekommen das Update"), "");
                    let _ = c.ui.upgrade_in_event_loop(|ui| ui.global::<AppState>().set_modal("".into()));
                    c.load_current_pack().await;
                }
                Err(e) => c.fail(e),
            }
            c.refresh();
        });
    }

    pub fn share(self: &C, id: String, days_idx: i32, uses_idx: i32) {
        let Some(pid) = self.inst(&id).and_then(|i| i.pack_id) else { return };
        let days = [0, 1, 7, 30][days_idx.clamp(0, 3) as usize];
        let uses = [0, 1, 5, 10][uses_idx.clamp(0, 3) as usize];
        self.with(|d| d.modal_busy = true);
        self.refresh();
        self.spawn(move |c| async move {
            match api::create_share(&pid, days, uses).await {
                Ok(s) => {
                    crate::copy_to_clipboard(&s.url);
                    c.with(|d| d.share = Some((s.code, s.url)));
                    c.toast("Link erstellt und kopiert", "");
                }
                Err(e) => c.fail(e),
            }
            c.with(|d| d.modal_busy = false);
            c.refresh();
        });
    }

    pub fn join_check(self: &C, code: String) {
        let code = parse_code(&code);
        if code.is_empty() {
            self.toast("Ungültiger Code", "warn");
            return;
        }
        if !api::logged_in() {
            self.toast("Bitte zuerst unter „Freunde“ anmelden", "warn");
            return;
        }
        self.with(|d| d.modal_busy = true);
        self.refresh();
        self.spawn(move |c| async move {
            match api::share_preview(&code).await {
                Ok(p) => c.with(|d| {
                    d.join_preview = format!(
                        "{}\nMinecraft {} · {} · von {} · {} Mitglied(er){}",
                        p.name,
                        p.minecraft,
                        loaders::display(&p.loader),
                        p.owner,
                        p.members,
                        if p.description.is_empty() { String::new() } else { format!("\n\n{}", p.description) }
                    );
                    d.join_ready = true;
                }),
                Err(e) => {
                    c.with(|d| {
                        d.join_preview.clear();
                        d.join_ready = false;
                    });
                    c.fail(e);
                }
            }
            c.with(|d| d.modal_busy = false);
            c.refresh();
        });
    }

    pub fn join(self: &C, code: String) {
        let code = parse_code(&code);
        self.with(|d| d.modal_busy = true);
        self.refresh();
        self.spawn(move |c| async move {
            let res = api::join(&code).await;
            c.with(|d| {
                d.modal_busy = false;
                d.join_ready = false;
                d.join_preview.clear();
            });
            match res {
                Ok(p) => {
                    let _ = c.ui.upgrade_in_event_loop(|ui| {
                        let s = ui.global::<AppState>();
                        s.set_modal("".into());
                        s.set_join_code("".into());
                    });
                    c.refresh_server().await;
                    c.install_pack(p.id, true);
                }
                Err(e) => c.fail(e),
            }
            c.refresh();
        });
    }

    fn pack_of(&self, inst_id: &str) -> Option<String> { self.inst(inst_id).and_then(|i| i.pack_id) }

    pub fn pack_action(self: &C, act: &'static str, inst_id: String, user_id: i64, role: String) {
        let Some(pid) = self.pack_of(&inst_id) else { return };
        let me = self.with(|d| d.user.as_ref().map(|u| u.id).unwrap_or(0));
        self.spawn(move |c| async move {
            let res = match act {
                "invite" => api::add_member(&pid, user_id).await.map(|_| "Eingeladen – das Pack erscheint jetzt bei deinem Freund"),
                "kick" => api::remove_member(&pid, user_id).await.map(|_| "Mitglied entfernt"),
                "role" => api::set_role(&pid, user_id, &role).await.map(|_| "Rolle geändert"),
                "leave" => api::remove_member(&pid, me).await.map(|_| "Pack verlassen"),
                "delete" => api::delete_pack(&pid).await.map(|_| "Pack gelöscht"),
                "force" => api::force_update(&pid).await.map(|_| "Update erzwungen – alle Mitglieder laden die neueste Version automatisch, sobald ihr Launcher läuft"),
                _ => return,
            };
            match res {
                Ok(msg) => {
                    if matches!(act, "leave" | "delete") {
                        if let Ok(i) = instances::unlink_pack(&inst_id) {
                            c.upsert(i);
                        }
                    }
                    c.toast(msg, "");
                    c.refresh_server().await;
                    c.load_current_pack().await;
                }
                Err(e) => c.fail(e),
            }
        });
    }

    pub fn auth(self: &C, register: bool, user: String, pass: String) {
        self.with(|d| d.modal_busy = true);
        self.refresh();
        self.spawn(move |c| async move {
            let res = if register { api::register(user.trim(), &pass).await } else { api::login(user.trim(), &pass).await };
            c.with(|d| d.modal_busy = false);
            match res {
                Ok(r) => {
                    c.want_image(&avatar_url(&r.user.username));
                    c.toast(format!("Willkommen, {}!", r.user.username), "");
                    c.set_session(Some(r.token), Some(r.user));
                    c.refresh_server().await;
                }
                Err(e) => c.fail(e),
            }
        });
    }

    pub fn logout(self: &C) {
        self.spawn(|c| async move {
            api::logout().await;
            c.set_session(None, None);
        });
    }

    pub fn friend_action(self: &C, act: &'static str, user_id: i64, name: String) {
        self.spawn(move |c| async move {
            let res = match act {
                "add" => api::friend_request(name.trim()).await.map(|s| {
                    if s == "accepted" { format!("Du bist jetzt mit {} befreundet", name.trim()) } else { format!("Anfrage an {} gesendet", name.trim()) }
                }),
                "accept" => api::friend_accept(user_id).await.map(|_| "Freundschaft angenommen".to_string()),
                "remove" => api::friend_remove(user_id).await.map(|_| "Entfernt".to_string()),
                _ => return,
            };
            match res {
                Ok(m) => c.toast(m, ""),
                Err(e) => c.fail(e),
            }
            c.refresh_server().await;
        });
    }

    pub fn notif_seen_all(self: &C) {
        self.with(|d| {
            let keys: Vec<String> = build_notifs(d).into_iter().map(|n| n.key).collect();
            for k in keys {
                if !d.notif.seen.contains(&k) {
                    d.notif.seen.push(k);
                }
            }
            d.last_unseen = 0;
            if let Some(u) = &d.user {
                settings::save_notifs(u.id, &d.notif);
            }
        });
        self.refresh();
    }

    pub fn notif_dismiss(self: &C, key: Option<String>) {
        self.with(|d| {
            let keys: Vec<String> = match key {
                Some(k) => vec![k],
                None => d.notif.events.iter().map(|e| e.key.clone()).collect(),
            };
            for k in keys {
                if !d.notif.dismissed.contains(&k) {
                    d.notif.dismissed.push(k);
                }
            }
            if let Some(u) = &d.user {
                settings::save_notifs(u.id, &d.notif);
            }
        });
        self.refresh();
    }

    pub fn install_update(self: &C) {
        let Some(info) = self.with(|d| d.update.clone()) else { return };
        self.with(|d| d.update_busy = true);
        self.refresh();
        self.spawn(move |c| async move {
            match updater::install(&info).await {
                Ok(()) => {
                    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
                    let _ = slint::invoke_from_event_loop(|| {
                        let _ = slint::quit_event_loop();
                    });
                }
                Err(e) => {
                    c.with(|d| d.update_busy = false);
                    c.fail(e);
                }
            }
        });
    }

    pub fn save_settings(self: &C, memory: String, jvm: String, launcher: String, close: bool) {
        let s = Settings {
            memory_mb: memory.trim().parse::<u32>().unwrap_or(4096).clamp(1024, 65536),
            jvm_args: jvm.trim().to_string(),
            official_launcher_path: launcher.trim().to_string(),
            close_on_launch: close,
        };
        settings::save(&s);
        self.with(|d| d.settings = s);
        self.toast("Einstellungen gespeichert", "");
    }

    pub fn settings(&self) -> Settings { self.with(|d| d.settings.clone()) }
}

pub fn parse_code(s: &str) -> String {
    let re = regex::Regex::new(r"([A-Za-z0-9]{6,16})/?\s*$").unwrap();
    re.captures(s.trim()).map(|c| c[1].to_string()).unwrap_or_default()
}

pub fn avatar_url(name: &str) -> String { format!("https://mc-heads.net/avatar/{}/64", urlencoding::encode(name)) }

fn decode(bytes: &[u8]) -> Option<Pixels> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = img.resize_to_fill(96, 96, image::imageops::FilterType::Triangle).to_rgba8();
    Some(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(img.as_raw(), img.width(), img.height()))
}

async fn load_image(url: &str) -> Option<Pixels> {
    let bytes = crate::net::get_bytes(url).await.ok()?;
    tokio::task::spawn_blocking(move || {
        let img = image::load_from_memory(&bytes).ok()?;
        let img = img.resize_to_fill(96, 96, image::imageops::FilterType::Triangle).to_rgba8();
        Some(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(img.as_raw(), img.width(), img.height()))
    })
    .await
    .ok()
    .flatten()
}

// ---------- Darstellung ----------

fn fmt_time(ts: i64) -> String {
    use chrono::TimeZone;
    if ts <= 0 {
        return String::new();
    }
    chrono::Local.timestamp_opt(ts, 0).single().map(|t| t.format("%d.%m.%Y, %H:%M").to_string()).unwrap_or_default()
}

fn fmt_num(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1} Mio.", n as f64 / 1e6).replacen('.', ",", 1)
    } else if n >= 1000 {
        format!("{:.1} Tsd.", n as f64 / 1e3).replacen('.', ",", 1)
    } else {
        n.to_string()
    }
}

fn tint(s: &str) -> slint::Color {
    let mut h: u32 = 0;
    for c in s.chars() {
        h = h.wrapping_mul(31).wrapping_add(c as u32);
    }
    hsl((h % 360) as f32, 0.65, 0.62)
}

fn hsl(h: f32, s: f32, l: f32) -> slint::Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    slint::Color::from_rgb_u8(((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8)
}

fn initial(name: &str) -> SharedString { name.trim().chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or("?".into()).into() }

fn image_of(d: &Data, url: Option<&str>) -> (slint::Image, bool) {
    match url.and_then(|u| d.images.get(u)) {
        Some(px) => (slint::Image::from_rgba8(px.clone()), true),
        None => (slint::Image::default(), false),
    }
}

fn inst_item(d: &Data, i: &Instance) -> InstanceItem {
    let pack = i.pack_id.as_ref().and_then(|p| d.packs.iter().find(|x| &x.id == p));
    let busy = d.busy.get(&i.id);
    InstanceItem {
        id: i.id.clone().into(),
        name: i.name.clone().into(),
        initial: initial(&i.name),
        tint: tint(&i.name),
        minecraft: i.manifest.minecraft.clone().into(),
        loader: loaders::display(&i.manifest.loader).into(),
        loader_version: i.manifest.loader_version.clone().unwrap_or_default().into(),
        mod_count: i.manifest.mods.len() as i32,
        pack_count: (i.manifest.resourcepacks.len() + i.manifest.shaders.len()) as i32,
        last_played: fmt_time(i.last_played).into(),
        shared: i.pack_id.is_some(),
        update: pack.map(|p| p.latest_version > i.pack_version).unwrap_or(false),
        dirty: i.dirty(),
        can_edit: i.can_edit(),
        role: i.pack_role.clone().unwrap_or_default().into(),
        busy: busy.is_some(),
        status: busy.map(|b| b.0.clone()).unwrap_or_default().into(),
        progress: busy.map(|b| b.1).unwrap_or(-1.0),
        pack_version: i.pack_version as i32,
        memory: i.memory_mb.map(|m| m.to_string()).unwrap_or_default().into(),
        mc_index: d.mc_versions.iter().position(|v| *v == i.manifest.minecraft).unwrap_or(0) as i32,
        loader_index: loaders::LOADERS.iter().position(|l| *l == i.manifest.loader).unwrap_or(0) as i32,
    }
}

struct Notif {
    key: String,
    kind: String,
    title: String,
    text: String,
    time: i64,
    unseen: bool,
    history: bool,
    a1: (String, String, String),
    a2: (String, String, String),
    user: Option<String>,
}

fn build_notifs(d: &Data) -> Vec<Notif> {
    let seen: HashSet<&String> = d.notif.seen.iter().collect();
    let mut out = Vec::new();
    let mut push = |key: String, kind: &str, title: String, text: String, time: i64, a1: (&str, &str, String), a2: (&str, &str, String), user: Option<String>, history: bool| {
        out.push(Notif {
            unseen: !seen.contains(&key),
            key,
            kind: kind.into(),
            title,
            text,
            time,
            history,
            a1: (a1.0.into(), a1.1.into(), a1.2),
            a2: (a2.0.into(), a2.1.into(), a2.2),
            user,
        })
    };
    let none = ("", "", String::new());
    if let Some(u) = d.update.as_ref().filter(|u| u.available) {
        push(
            format!("app:{}", u.latest),
            "app-update",
            format!("NNN Launcher {} ist da", u.latest),
            format!("Du hast Version {}. Das Update dauert nur ein paar Sekunden.", crate::net::VERSION),
            0,
            ("Jetzt aktualisieren", "app-update", String::new()),
            none.clone(),
            None,
            false,
        );
    }
    for f in &d.friends.incoming {
        push(
            format!("fr:{}", f.id),
            "friend",
            format!("{} möchte mit dir befreundet sein", f.username),
            String::new(),
            f.since,
            ("Annehmen", "accept-friend", f.id.to_string()),
            ("Ablehnen", "remove-friend", f.id.to_string()),
            Some(f.username.clone()),
            false,
        );
    }
    for i in &d.instances {
        let Some(p) = i.pack_id.as_ref().and_then(|p| d.packs.iter().find(|x| &x.id == p)) else { continue };
        if p.latest_version > i.pack_version {
            push(
                format!("up:{}:{}", p.id, p.latest_version),
                "update",
                format!("Update für „{}“", i.name),
                format!("Version {} ist verfügbar (du hast {}).", p.latest_version, i.pack_version),
                p.updated_at,
                ("Aktualisieren", "sync", i.id.clone()),
                ("Ansehen", "open", i.id.clone()),
                None,
                false,
            );
        } else if i.dirty() && matches!(p.role.as_str(), "owner" | "editor") {
            push(
                format!("dirty:{}:{}:{}", i.id, i.manifest.mods.len(), i.pack_version),
                "dirty",
                format!("„{}“ hat unveröffentlichte Änderungen", i.name),
                "Deine Freunde haben diese Mods noch nicht.".into(),
                0,
                ("Veröffentlichen", "push", i.id.clone()),
                none.clone(),
                None,
                false,
            );
        }
    }
    let me = d.user.as_ref().map(|u| u.id).unwrap_or(0);
    for p in &d.packs {
        if p.owner.id == me || d.instances.iter().any(|i| i.pack_id.as_deref() == Some(p.id.as_str())) {
            continue;
        }
        push(
            format!("inv:{}", p.id),
            "invite",
            format!("Du bist in „{}“", p.name),
            format!("Modpack von {} · Minecraft {} · {}", p.owner.username, p.minecraft, loaders::display(&p.loader)),
            p.created_at,
            ("Installieren", "install-pack", p.id.clone()),
            none.clone(),
            None,
            false,
        );
    }
    let dismissed: HashSet<&String> = d.notif.dismissed.iter().collect();
    for e in d.notif.events.iter().filter(|e| !dismissed.contains(&e.key)) {
        push(e.key.clone(), &e.kind, e.title.clone(), String::new(), e.time, none.clone(), none.clone(), (!e.user.is_empty()).then(|| e.user.clone()), true);
    }
    out
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> { ModelRc::new(VecModel::from(v)) }
fn strings(v: impl IntoIterator<Item = String>) -> ModelRc<SharedString> { model(v.into_iter().map(SharedString::from).collect()) }

/// Spiegelt den Zustand in die Oberfläche (läuft im UI-Thread)
fn apply(c: &C, ui: &AppWindow, d: &Data) {
    let s = ui.global::<AppState>();
    let user = d.user.as_ref();
    s.set_logged_in(user.is_some());
    s.set_username(user.map(|u| u.username.clone()).unwrap_or_default().into());
    s.set_server_online(d.server_online);
    let (av, has) = image_of(d, user.map(|u| avatar_url(&u.username)).as_deref());
    s.set_my_avatar(av);
    s.set_has_my_avatar(has);

    // Bibliothek: zuletzt gespielte Instanz groß oben
    let mut list: Vec<&Instance> = d.instances.iter().collect();
    list.sort_by_key(|i| std::cmp::Reverse((i.last_played, i.created_at)));
    let items: Vec<InstanceItem> = list.iter().map(|i| inst_item(d, i)).collect();
    s.set_has_hero(!items.is_empty());
    if let Some(h) = items.first() {
        s.set_hero(h.clone());
    }
    s.set_instances(model(items));

    let installed: HashSet<&str> = d.instances.iter().filter_map(|i| i.pack_id.as_deref()).collect();
    let open: Vec<PackItem> = d
        .packs
        .iter()
        .filter(|p| !installed.contains(p.id.as_str()))
        .map(|p| {
            let b = d.busy.get(&format!("pack:{}", p.id));
            PackItem {
                id: p.id.clone().into(),
                name: p.name.clone().into(),
                sub: format!("{} · {} · von {} · Version {}", p.minecraft, loaders::display(&p.loader), p.owner.username, p.latest_version).into(),
                initial: initial(&p.name),
                tint: tint(&p.name),
                busy: b.is_some(),
                progress: b.map(|b| b.1).unwrap_or(-1.0),
            }
        })
        .collect();
    s.set_open_packs(model(open));

    // Detail
    if let Some(cur) = d.current.as_ref().and_then(|id| d.instances.iter().find(|i| &i.id == id)) {
        s.set_current(inst_item(d, cur));
        // Suchfeld im Inhalte-Tab: Name, Datei oder Version
        let filter = s.get_content_filter().to_lowercase();
        let matches = |name: &str, file: &str, version: &str| {
            filter.is_empty() || [name, file, version].iter().any(|x| x.to_lowercase().contains(filter.trim()))
        };
        let total: usize = Kind::ALL.iter().map(|k| cur.manifest.list(*k).len()).sum();
        let items = |k: Kind| {
            let mut v = cur.manifest.list(k).clone();
            v.retain(|m| matches(&m.name, &m.file_name, m.version_name.as_deref().unwrap_or("")));
            v.sort_by_key(|m| m.name.to_lowercase());
            model(
                v.iter()
                    .map(|m| {
                        let (icon, has_icon) = image_of(d, m.icon_url.as_deref());
                        ModItem {
                            name: m.name.clone().into(),
                            version: (if m.source == "p2p" && instances::existing_path(&cur.id, k, &m.file_name).is_none() {
                                "wartet auf Freund (online)".to_string()
                            } else {
                                m.version_name.clone().unwrap_or_else(|| m.file_name.clone())
                            })
                            .into(),
                            source: (match m.source.as_str() { "curseforge" => "CurseForge", "p2p" => "Freunde (P2P)", _ => "Modrinth" }).into(),
                            file: m.file_name.clone().into(),
                            kind: k.key().into(),
                            dependency: m.dependency,
                            enabled: !instances::is_disabled(&cur.id, k, &m.file_name),
                            local: false,
                            icon,
                            has_icon,
                        }
                    })
                    .collect(),
            )
        };
        let mods_m = items(Kind::Mod);
        let packs_m = items(Kind::ResourcePack);
        let shaders_m = items(Kind::Shader);
        let shown = mods_m.row_count() + packs_m.row_count() + shaders_m.row_count();
        let off: usize = Kind::ALL.iter().map(|k| cur.manifest.list(*k).iter().filter(|m| instances::is_disabled(&cur.id, *k, &m.file_name)).count()).sum();
        s.set_content_info(
            (if filter.trim().is_empty() {
                format!("{} Mods · {} Texturepacks · {} Shader{}", cur.manifest.mods.len(), cur.manifest.resourcepacks.len(), cur.manifest.shaders.len(), if off > 0 { format!(" · {off} ausgeschaltet") } else { String::new() })
            } else {
                format!("{shown} von {total} Einträgen passen zu „{}“", filter.trim())
            })
            .into(),
        );
        let local_total: usize = Kind::ALL.iter().map(|k| instances::local_files(cur, *k).len()).sum();
        s.set_mods(mods_m);
        s.set_resourcepacks(packs_m);
        s.set_shaders(shaders_m);
        s.set_has_shader_loader(cur.has_shader_loader());
        let found = d.scan_found.get(&cur.id);
        let local: Vec<ModItem> = Kind::ALL
            .iter()
            .flat_map(|k| instances::local_files(cur, *k).into_iter().map(move |f| (*k, f)))
            .map(|(k, f)| {
                let info = found.and_then(|v| v.iter().find(|x| x.file_name.eq_ignore_ascii_case(&f)));
                let entry = info.and_then(|i| i.entry.as_ref());
                let (icon, has_icon) = match entry.and_then(|e| e.icon_url.as_deref()) {
                    Some(u) => image_of(d, Some(u)),
                    None => image_of(d, Some(&format!("local:{}/{}", cur.id, f))),
                };
                let name = entry.map(|e| e.name.clone()).or_else(|| info.map(|i| i.meta_name.clone()).filter(|n| !n.is_empty())).unwrap_or_else(|| f.clone());
                let version = entry
                    .and_then(|e| e.version_name.clone())
                    .or_else(|| info.map(|i| i.meta_version.clone()).filter(|v| !v.is_empty()))
                    .unwrap_or_default();
                let origin = if entry.is_some() { "Modrinth" } else { "nicht erkannt" };
                let enabled = !instances::is_disabled(&cur.id, k, &f);
                ModItem {
                    name: name.into(),
                    version: version.into(),
                    source: format!("{} · {origin}", k.label()).into(),
                    file: f.into(),
                    kind: k.key().into(),
                    dependency: false,
                    enabled,
                    local: true,
                    icon,
                    has_icon,
                }
            })
            .filter(|m: &ModItem| matches(&m.name, &m.file, &m.version))
            .collect();
        if !filter.trim().is_empty() {
            s.set_content_info(
                format!("{} von {} Einträgen passen zu „{}“", shown + local.len(), total + local_total, filter.trim()).into(),
            );
        }
        s.set_local_count(local_total as i32);
        s.set_local_items(model(local));
        s.set_scanning(d.scanning.contains(&cur.id));
        let me = user.map(|u| u.id).unwrap_or(0);
        match &d.current_pack {
            Some(p) => {
                s.set_pack_loaded(true);
                s.set_pack_name(p.name.clone().into());
                let role = match p.role.as_str() { "owner" => "Besitzer", "editor" => "Editor", _ => "Mitglied" };
                let forced = if p.force_version > 0 && p.force_version >= p.latest_version {
                    format!(" · Update auf Version {} erzwungen{}", p.force_version, if p.force_by.is_empty() { String::new() } else { format!(" von {}", p.force_by) })
                } else {
                    String::new()
                };
                s.set_pack_info(format!("Version {} · Besitzer {} · deine Rolle: {}{forced}", p.latest_version, p.owner.username, role).into());
                let current_count = p.members.iter().filter(|m| m.installed_version >= p.latest_version).count();
                s.set_members_info(format!("{current_count} von {} auf dem neuesten Stand", p.members.len()).into());
                s.set_is_owner(p.role == "owner");
                s.set_members(model(
                    p.members
                        .iter()
                        .map(|m| {
                            let (avatar, has_avatar) = image_of(d, Some(&avatar_url(&m.username)));
                            MemberItem {
                                id: m.id as i32,
                                name: m.username.clone().into(),
                                role: m.role.clone().into(),
                                role_label: (match m.role.as_str() { "owner" => "Besitzer", "editor" => "Editor", _ => "Mitglied" }).into(),
                                is_me: m.id == me,
                                installed: (if m.installed_version > 0 { format!("v{}", m.installed_version) } else { "–".into() }).into(),
                                installed_ok: m.installed_version >= p.latest_version,
                                installed_known: m.installed_version > 0,
                                installed_when: fmt_time(m.installed_at).into(),
                                avatar,
                                has_avatar,
                            }
                        })
                        .collect(),
                ));
            }
            None => {
                s.set_pack_loaded(cur.pack_id.is_none());
                s.set_pack_name(cur.name.clone().into());
                s.set_pack_info("".into());
                s.set_members_info("".into());
                s.set_is_owner(cur.pack_role.as_deref() == Some("owner"));
                s.set_members(model(vec![]));
            }
        }
        s.set_versions(model(
            d.current_versions
                .iter()
                .map(|v| VersionItem {
                    title: format!("Version {}", v.version).into(),
                    sub: format!("{} · {}", v.author, fmt_time(v.created_at)).into(),
                    changelog: v.changelog.clone().into(),
                    installed: v.version == cur.pack_version,
                })
                .collect(),
        ));
    }

    // Versionen
    s.set_mc_versions(strings(d.mc_versions.clone()));
    s.set_loader_versions(strings(d.loader_versions.iter().map(|l| l.version.clone())));
    s.set_loader_labels(strings(d.loader_versions.iter().map(|l| l.label.clone())));
    s.set_loader_default(d.loader_versions.iter().position(|l| !l.label.contains("beta")).unwrap_or(0) as i32);
    s.set_loader_versions_loading(d.loader_loading);

    // Mods finden
    // Alle Instanzen – Texturepacks gehen auch bei Vanilla
    let targets: Vec<&Instance> = d.instances.iter().collect();
    s.set_targets(strings(targets.iter().map(|i| format!("{} ({} {})", i.name, i.manifest.minecraft, loaders::display(&i.manifest.loader)))));
    s.set_target_ids(strings(targets.iter().map(|i| i.id.clone())));
    if s.get_target() as usize >= targets.len() {
        s.set_target(0);
    }
    let target = targets.get(s.get_target().max(0) as usize).copied();
    let have: HashSet<String> = target
        .map(|t| t.manifest.list(d.hits_kind).iter().map(|m| format!("{}:{}", m.source, m.project_id)).collect())
        .unwrap_or_default();
    s.set_hits(model(
        d.hits
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let (icon, has_icon) = image_of(d, h.icon.as_deref());
                HitItem {
                    title: h.title.clone().into(),
                    author: h.author.clone().into(),
                    description: h.description.clone().into(),
                    downloads: fmt_num(h.downloads).into(),
                    url: h.url.clone().into(),
                    icon,
                    has_icon,
                    installed: have.contains(&format!("{}:{}", h.source, h.id)),
                    busy: d.hit_busy.contains(&i),
                }
            })
            .collect(),
    ));
    s.set_hits_loading(d.hits_loading);
    s.set_hits_error(d.hits_error.clone().into());
    s.set_hits_more(d.hits.len() < d.hits_total && !d.hits.is_empty());

    // Freunde
    let users = |list: &Vec<api::Friend>| -> ModelRc<UserItem> {
        model(
            list.iter()
                .map(|f| {
                    let (avatar, has_avatar) = image_of(d, Some(&avatar_url(&f.username)));
                    UserItem { id: f.id as i32, name: f.username.clone().into(), sub: "".into(), avatar, has_avatar }
                })
                .collect(),
        )
    };
    s.set_friends(users(&d.friends.friends));
    s.set_incoming(users(&d.friends.incoming));
    s.set_outgoing(users(&d.friends.outgoing));
    s.set_incoming_count(d.friends.incoming.len() as i32);
    s.set_friend_names(strings(d.friends.friends.iter().map(|f| f.username.clone())));

    // Benachrichtigungen
    let notifs = build_notifs(d);
    s.set_unseen(notifs.iter().filter(|n| n.unseen).count() as i32);
    s.set_bell_ring(d.bell_ring);
    s.set_notifications(model(
        notifs
            .iter()
            .map(|n| {
                let (avatar, has_avatar) = image_of(d, n.user.as_ref().map(|u| avatar_url(u)).as_deref());
                NotifItem {
                    key: n.key.clone().into(),
                    kind: n.kind.clone().into(),
                    title: n.title.clone().into(),
                    text: n.text.clone().into(),
                    time: fmt_time(n.time).into(),
                    unseen: n.unseen,
                    history: n.history,
                    a1_label: n.a1.0.clone().into(),
                    a1_act: n.a1.1.clone().into(),
                    a1_arg: n.a1.2.clone().into(),
                    a2_label: n.a2.0.clone().into(),
                    a2_act: n.a2.1.clone().into(),
                    a2_arg: n.a2.2.clone().into(),
                    avatar,
                    has_avatar,
                }
            })
            .collect(),
    ));

    // Update
    let up = d.update.as_ref().filter(|u| u.available);
    s.set_update_available(up.is_some());
    s.set_update_version(up.map(|u| u.latest.clone()).unwrap_or_default().into());
    s.set_update_notes(up.map(|u| u.notes.clone()).unwrap_or_default().into());
    s.set_update_state(d.update_state.clone().into());
    s.set_update_busy(d.update_busy);

    // Dialoge
    s.set_modal_busy(d.modal_busy);
    s.set_share_code(d.share.as_ref().map(|x| x.0.clone()).unwrap_or_default().into());
    s.set_share_url(d.share.as_ref().map(|x| x.1.clone()).unwrap_or_default().into());
    s.set_join_preview(d.join_preview.clone().into());
    s.set_join_ready(d.join_ready);

    s.set_toasts(model(d.toasts.iter().map(|t| ToastItem { id: t.0, text: t.1.clone().into(), kind: t.2.clone().into() }).collect()));

    // Avatare für sichtbare Personen nachladen
    for f in d.friends.friends.iter().chain(&d.friends.incoming).chain(&d.friends.outgoing) {
        c.want_image(&avatar_url(&f.username));
    }
    if let Some(u) = user {
        c.want_image(&avatar_url(&u.username));
    }
}
