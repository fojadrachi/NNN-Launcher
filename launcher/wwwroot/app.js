'use strict';
// NNN Launcher – Oberfläche. Spricht per Bridge mit dem C#-Kern, direkt mit Modrinth
// und mit dem NNN-Server (Freunde, Packs, Share-Links, CurseForge-Proxy).

// ================= Bridge =================

const bridge = (() => {
  const pending = new Map();
  const listeners = {};
  let seq = 0;
  const wv = window.chrome && window.chrome.webview;
  if (wv) {
    wv.addEventListener('message', (e) => {
      const m = e.data;
      if (m && m.id && pending.has(m.id)) {
        const { res, rej } = pending.get(m.id);
        pending.delete(m.id);
        m.error !== undefined && m.error !== null ? rej(new Error(m.error)) : res(m.result);
      } else if (m && m.event) {
        (listeners[m.event] || []).forEach((cb) => cb(m.data));
      }
    });
  }
  return {
    call(method, params = {}) {
      if (!wv) return mockCall(method, params);
      return new Promise((res, rej) => {
        const id = String(++seq);
        pending.set(id, { res, rej });
        wv.postMessage({ id, method, params });
      });
    },
    on(event, cb) { (listeners[event] ||= []).push(cb); },
  };
})();
const call = bridge.call;

// Nur für die Vorschau im normalen Browser (ohne C#-Kern)
const mockMods = (n) => ["Create", "JEI", "Jade", "Farmer's Delight", "Sodium", "Iris Shaders", "Xaero's Minimap", "Waystones", "Supplementaries", "Biomes O' Plenty"].slice(0, Math.min(n, 10)).map((name, i) => ({ name, fileName: name.toLowerCase().replace(/W+/g, "-") + ".jar", versionName: "1." + i + ".0", source: i % 3 ? "modrinth" : "curseforge", projectId: String(i), dependency: i === 3 }));
function mockCall(method, p) {
  const db = (window.__mock ||= {
    settings: { serverUrl: 'https://nnn-launcher.christianolah56-42e.workers.dev', msClientId: '', memoryMb: 4096, jvmArgs: '', showSnapshots: false, closeOnLaunch: false },
    session: { token: null, user: null },
    accounts: { selected: 'a', accounts: [{ id: 'a', name: 'Fojadrachi', type: 'offline' }] },
    instances: [
      { id: 'survival', name: 'Survival mit Freunden', minecraft: '1.20.1', loader: 'forge', loaderVersion: '47.4.28', mods: mockMods(42), packId: 'x', packVersion: 3, packRole: 'owner', dirty: false, createdAt: 0, lastPlayed: 1793300000 },
      { id: 'create', name: 'Create Abenteuer', minecraft: '1.21.1', loader: 'neoforge', loaderVersion: '21.1.265', mods: mockMods(18), packId: null, packVersion: 0, packRole: null, dirty: false, createdAt: 0, lastPlayed: 1793200000 },
      { id: 'pvp', name: 'PvP Optimiert', minecraft: '1.20.1', loader: 'fabric', loaderVersion: '0.20.1', mods: mockMods(7), packId: null, packVersion: 0, packRole: null, dirty: false, createdAt: 0, lastPlayed: 0 },
    ],
  });
  const r = {
    'app.ready': { version: '1.0.0 (Vorschau)' },
    'settings.get': db.settings, 'session.get': db.session, 'accounts.list': db.accounts,
    'instances.list': db.instances, 'instances.localMods': [], 'game.running': [],
    'mc.versions': ['1.21.1', '1.20.4', '1.20.1', '1.19.2', '1.18.2'].map((id) => ({ id, type: 'release' })),
    'loader.versions': [{ version: '0.16.9', stable: true }],
  };
  if (method === 'session.set') { db.session = p; return Promise.resolve(true); }
  if (method === 'settings.set') { db.settings = p; return Promise.resolve(p); }
  if (method === 'instances.get') return Promise.resolve(db.instances.find((i) => i.id === p.id));
  if (method in r) return Promise.resolve(r[method]);
  return Promise.reject(new Error('In der Browser-Vorschau nicht verfügbar'));
}

// ================= Zustand =================

const S = {
  view: 'library',
  instanceId: null,
  tab: 'mods',
  settings: {},
  session: { token: null, user: null },
  accounts: { accounts: [], selected: null },
  instances: [],
  packs: [],
  friends: { friends: [], incoming: [], outgoing: [] },
  progress: {},
  logs: {},
  running: new Set(),
  serverOk: null,
  notifiedUpdates: new Set(),
  discover: { source: 'modrinth', query: '', target: null, results: [], loading: false, error: null, offset: 0, total: 0 },
  mcVersions: [],
};

const LOADER_NAMES = { vanilla: 'Vanilla', fabric: 'Fabric', quilt: 'Quilt', forge: 'Forge', neoforge: 'NeoForge' };
const $ = (sel, root = document) => root.querySelector(sel);
const main = $('#main');

function esc(s) {
  return String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}
function fmtNum(n) {
  if (n >= 1e6) return (n / 1e6).toFixed(1).replace('.', ',') + ' Mio.';
  if (n >= 1e3) return (n / 1e3).toFixed(1).replace('.', ',') + ' Tsd.';
  return String(n);
}
function fmtDate(sec) {
  if (!sec) return '–';
  return new Date(sec * 1000).toLocaleString('de-DE', { dateStyle: 'medium', timeStyle: 'short' });
}
function colorFor(s) {
  let h = 0;
  for (const c of String(s)) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return `hsl(${h % 360} 65% 62%)`;
}
function instIcon(name) {
  return `<div class="inst-icon" style="background:${colorFor(name)}">${esc((name || '?').trim()[0]?.toUpperCase() || '?')}</div>`;
}
const ICON = {
  play: '<svg viewBox="0 0 24 24"><path d="M7 4l13 8-13 8z" fill="currentColor" stroke="none"/></svg>',
  stop: '<svg viewBox="0 0 24 24"><rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor" stroke="none"/></svg>',
  plus: '<svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14"/></svg>',
  link: '<svg viewBox="0 0 24 24"><path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/></svg>',
  sync: '<svg viewBox="0 0 24 24"><path d="M20 12a8 8 0 0 1-14 5.3M4 12a8 8 0 0 1 14-5.3"/><path d="M18 3v4h-4M6 21v-4h4"/></svg>',
  upload: '<svg viewBox="0 0 24 24"><path d="M12 16V4M6 10l6-6 6 6M4 20h16"/></svg>',
  trash: '<svg viewBox="0 0 24 24"><path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/></svg>',
  folder: '<svg viewBox="0 0 24 24"><path d="M3 6h6l2 2h10v11H3z"/></svg>',
  back: '<svg viewBox="0 0 24 24"><path d="M15 18l-6-6 6-6"/></svg>',
  users: '<svg viewBox="0 0 24 24"><circle cx="9" cy="8" r="3.5"/><path d="M2.5 20c.8-3.6 3.4-5.5 6.5-5.5s5.7 1.9 6.5 5.5"/></svg>',
  box: '<svg viewBox="0 0 24 24"><path d="M4 7l8-4 8 4v10l-8 4-8-4z"/><path d="M4 7l8 4 8-4M12 11v10"/></svg>',
  copy: '<svg viewBox="0 0 24 24"><rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V4H4v12h4"/></svg>',
  join: '<svg viewBox="0 0 24 24"><path d="M15 3h5v18h-5M10 17l5-5-5-5M15 12H3"/></svg>',
  check: '<svg viewBox="0 0 24 24"><path d="M5 12l5 5 9-10"/></svg>',
  x: '<svg viewBox="0 0 24 24"><path d="M6 6l12 12M18 6L6 18"/></svg>',
};

// ================= Toasts & Modals =================

function toast(msg, kind = '') {
  const el = document.createElement('div');
  el.className = 'toast ' + kind;
  el.textContent = msg;
  $('#toasts').appendChild(el);
  setTimeout(() => el.remove(), kind === 'error' ? 7000 : 4000);
}
const fail = (e) => toast(e.message || String(e), 'error');

function modal(html, { wide = false, onClose } = {}) {
  const back = document.createElement('div');
  back.className = 'modal-back';
  back.innerHTML = `<div class="modal ${wide ? 'wide' : ''}">${html}</div>`;
  const close = () => { back.remove(); onClose && onClose(); };
  back.addEventListener('mousedown', (e) => { if (e.target === back) close(); });
  back.addEventListener('click', (e) => { if (e.target.closest('[data-close]')) close(); });
  $('#modalRoot').appendChild(back);
  const first = back.querySelector('input:not([type=checkbox]), textarea, select');
  if (first) setTimeout(() => first.focus(), 30);
  return { el: back.firstElementChild, close };
}

function confirmDialog(title, text, okLabel = 'OK', danger = false) {
  return new Promise((resolve) => {
    let done = false;
    const m = modal(`<h2>${esc(title)}</h2><p class="muted">${esc(text)}</p>
      <div class="actions"><button class="btn" data-close>Abbrechen</button>
      <button class="btn ${danger ? 'danger solid' : 'primary'}" id="okBtn">${esc(okLabel)}</button></div>`,
      { onClose: () => { if (!done) resolve(false); } });
    $('#okBtn', m.el).onclick = () => { done = true; m.close(); resolve(true); };
  });
}

async function busy(btn, fn) {
  const old = btn && btn.innerHTML;
  if (btn) { btn.disabled = true; btn.innerHTML = '<span class="spinner"></span>'; }
  try { return await fn(); } finally { if (btn && btn.isConnected) { btn.disabled = false; btn.innerHTML = old; } }
}

async function copy(text) {
  try { await navigator.clipboard.writeText(text); toast('Kopiert'); } catch { toast('Kopieren fehlgeschlagen', 'error'); }
}

// ================= Server-API =================

async function api(path, { method = 'GET', body } = {}) {
  const headers = { 'Content-Type': 'application/json' };
  if (S.session.token) headers.Authorization = 'Bearer ' + S.session.token;
  let res;
  try {
    res = await fetch(S.settings.serverUrl + path, { method, headers, body: body ? JSON.stringify(body) : undefined });
  } catch {
    setServerOk(false);
    throw new Error('NNN-Server nicht erreichbar');
  }
  setServerOk(true);
  let data = {};
  try { data = await res.json(); } catch {}
  if (res.status === 401 && S.session.token && !path.startsWith('/auth')) {
    await setSession(null, null);
    render();
  }
  if (!res.ok) throw new Error(data.error || `Serverfehler (${res.status})`);
  return data;
}

function setServerOk(ok) {
  if (S.serverOk === ok) return;
  S.serverOk = ok;
  renderSidebar();
}

async function setSession(token, user) {
  S.session = { token, user };
  await call('session.set', { token, user });
}

const loggedIn = () => !!S.session.token;

// ================= Modrinth & CurseForge =================

const MR = 'https://api.modrinth.com/v2';

function mrLoaders(loader, mc) {
  switch (loader) {
    case 'fabric': return ['fabric'];
    case 'quilt': return ['quilt', 'fabric'];
    case 'forge': return ['forge'];
    case 'neoforge': return mc === '1.20.1' ? ['neoforge', 'forge'] : ['neoforge'];
    default: return [];
  }
}
const CF_LOADER = { forge: 1, fabric: 4, quilt: 5, neoforge: 6 };

async function mrGet(path) {
  const res = await fetch(MR + path);
  if (!res.ok) throw new Error(`Modrinth-Fehler (${res.status})`);
  return res.json();
}

const sources = {
  modrinth: {
    async search(q, inst, offset) {
      const facets = [['project_type:mod']];
      if (inst) {
        const l = mrLoaders(inst.loader, inst.minecraft);
        if (l.length) facets.push(l.map((x) => `categories:${x}`));
        facets.push([`versions:${inst.minecraft}`]);
      }
      const params = new URLSearchParams({ query: q, limit: '20', offset: String(offset), index: q ? 'relevance' : 'downloads', facets: JSON.stringify(facets) });
      const data = await mrGet('/search?' + params);
      return {
        total: data.total_hits,
        items: data.hits.map((h) => ({
          source: 'modrinth', id: h.project_id, title: h.title, description: h.description, icon: h.icon_url,
          downloads: h.downloads, author: h.author, url: `https://modrinth.com/mod/${h.slug}`,
        })),
      };
    },
    async resolve(projectId, inst, title, icon) {
      const loaders = mrLoaders(inst.loader, inst.minecraft);
      const params = new URLSearchParams({ game_versions: JSON.stringify([inst.minecraft]) });
      if (loaders.length) params.set('loaders', JSON.stringify(loaders));
      const versions = await mrGet(`/project/${projectId}/version?${params}`);
      if (!versions.length) throw new Error(`${title || projectId}: keine Version für ${inst.minecraft} / ${LOADER_NAMES[inst.loader]}`);
      // Bevorzugt: passender Haupt-Loader, dann stabile Version
      const pick = versions.find((v) => v.version_type === 'release' && v.loaders.includes(loaders[0]))
        || versions.find((v) => v.loaders.includes(loaders[0])) || versions[0];
      return mrEntry(pick, title, icon);
    },
    async deps(entry, inst) {
      const v = await mrGet(`/version/${entry.versionId}`);
      const out = [];
      for (const d of v.dependencies || []) {
        if (d.dependency_type !== 'required' || !d.project_id) continue;
        const p = await mrGet(`/project/${d.project_id}`);
        if (p.project_type !== 'mod') continue;
        out.push(await sources.modrinth.resolve(d.project_id, inst, p.title, p.icon_url));
      }
      return out;
    },
  },
  curseforge: {
    async search(q, inst, offset) {
      const params = new URLSearchParams({ gameId: '432', classId: '6', searchFilter: q, sortField: q ? '1' : '2', sortOrder: 'desc', pageSize: '20', index: String(offset) });
      if (inst) {
        params.set('gameVersion', inst.minecraft);
        if (CF_LOADER[inst.loader]) params.set('modLoaderType', String(CF_LOADER[inst.loader]));
      }
      const data = await api('/cf/v1/mods/search?' + params);
      return {
        total: Math.min(data.pagination?.totalCount || 0, 10000),
        items: data.data.map((m) => ({
          source: 'curseforge', id: String(m.id), title: m.name, description: m.summary, icon: m.logo?.thumbnailUrl,
          downloads: m.downloadCount, author: m.authors?.[0]?.name, url: m.links?.websiteUrl,
        })),
      };
    },
    async resolve(modId, inst, title, icon) {
      const tryType = async (type) => {
        const params = new URLSearchParams({ gameVersion: inst.minecraft, pageSize: '30' });
        if (type) params.set('modLoaderType', String(type));
        return (await api(`/cf/v1/mods/${modId}/files?${params}`)).data;
      };
      let files = await tryType(CF_LOADER[inst.loader]);
      if (!files.length && inst.loader === 'neoforge' && inst.minecraft === '1.20.1') files = await tryType(1);
      if (!files.length && inst.loader === 'quilt') files = await tryType(4);
      if (!files.length) throw new Error(`${title || modId}: keine Datei für ${inst.minecraft} / ${LOADER_NAMES[inst.loader]}`);
      files.sort((a, b) => new Date(b.fileDate) - new Date(a.fileDate));
      const f = files.find((x) => x.releaseType === 1) || files[0];
      if (!title) {
        const m = (await api(`/cf/v1/mods/${modId}`)).data;
        title = m.name; icon = m.logo?.thumbnailUrl;
      }
      if (!f.downloadUrl) throw new Error(`${title}: Der Autor erlaubt keine Downloads über andere Launcher.`);
      return {
        source: 'curseforge', projectId: String(modId), versionId: String(f.id), name: title, versionName: f.displayName,
        fileName: f.fileName, url: f.downloadUrl, sha1: (f.hashes || []).find((h) => h.algo === 1)?.value || null,
        size: f.fileLength || 0, iconUrl: icon || null, _deps: (f.dependencies || []).filter((d) => d.relationType === 3).map((d) => d.modId),
      };
    },
    async deps(entry, inst) {
      const out = [];
      for (const id of entry._deps || []) out.push(await sources.curseforge.resolve(String(id), inst));
      return out;
    },
  },
};

function mrEntry(v, title, icon) {
  const f = v.files.find((x) => x.primary) || v.files[0];
  return {
    source: 'modrinth', projectId: v.project_id, versionId: v.id, name: title || v.name, versionName: v.version_number,
    fileName: f.filename, url: f.url, sha1: f.hashes?.sha1 || null, size: f.size || 0, iconUrl: icon || null,
  };
}

/// Mod + benötigte Abhängigkeiten auflösen und installieren
async function installMod(item, inst) {
  const src = sources[item.source];
  const have = new Set(inst.mods.map((m) => m.source + ':' + m.projectId));
  const main = await src.resolve(item.id, inst, item.title, item.icon);
  const list = [main];
  const seen = new Set([main.source + ':' + main.projectId]);
  const queue = [main];
  while (queue.length) {
    const cur = queue.shift();
    let deps = [];
    try { deps = await src.deps(cur, inst); } catch (e) { toast(`Abhängigkeit übersprungen: ${e.message}`, 'warn'); }
    for (const d of deps) {
      const key = d.source + ':' + d.projectId;
      if (seen.has(key) || have.has(key)) continue;
      seen.add(key);
      d.dependency = true;
      list.push(d);
      queue.push(d);
    }
  }
  list.forEach((m) => delete m._deps);
  const updated = await call('instances.addMods', { id: inst.id, mods: list });
  upsertInstance(updated);
  toast(list.length > 1 ? `${main.name} + ${list.length - 1} Abhängigkeit(en) installiert` : `${main.name} installiert`);
  return updated;
}

// ================= Daten laden =================

function upsertInstance(inst) {
  const i = S.instances.findIndex((x) => x.id === inst.id);
  if (i >= 0) S.instances[i] = inst; else S.instances.unshift(inst);
}
const getInst = (id) => S.instances.find((i) => i.id === id);
const packFor = (inst) => inst && inst.packId && S.packs.find((p) => p.id === inst.packId);
const hasUpdate = (inst) => { const p = packFor(inst); return p && p.latestVersion > inst.packVersion; };

async function refreshInstances() { S.instances = await call('instances.list'); }

async function refreshPacks(silent = true) {
  if (!loggedIn()) { S.packs = []; return; }
  try {
    S.packs = (await api('/packs')).packs;
    for (const inst of S.instances) {
      const p = packFor(inst);
      if (p && p.latestVersion > inst.packVersion) {
        const key = p.id + ':' + p.latestVersion;
        if (!S.notifiedUpdates.has(key)) {
          S.notifiedUpdates.add(key);
          if (!silent || S.notifiedUpdates.size > 0) toast(`Update für „${p.name}“ verfügbar (Version ${p.latestVersion})`);
        }
      }
    }
  } catch (e) { if (!silent) fail(e); }
}

async function refreshFriends() {
  if (!loggedIn()) { S.friends = { friends: [], incoming: [], outgoing: [] }; return; }
  try {
    S.friends = await api('/friends');
    // Wer aus "Gesendet" jetzt Freund ist, hat angenommen (auch während der Launcher zu war)
    const me = S.session.user?.id;
    const key = 'nnn.outgoing.' + me;
    const before = store.get(key, []);
    const friendIds = new Set(S.friends.friends.map((f) => f.id));
    for (const o of before) if (friendIds.has(o.id)) addEvent({ kind: 'friend-accepted', key: 'fa:' + o.id, title: `${o.username} hat deine Freundschaftsanfrage angenommen`, user: o.username });
    store.set(key, S.friends.outgoing.map((o) => ({ id: o.id, username: o.username })));
  } catch {}
}

async function refreshAll() {
  await Promise.all([refreshInstances(), refreshPacks(), refreshFriends()]);
  render();
}

// ================= Benachrichtigungen =================
// Aktuelle Meldungen werden aus dem Zustand abgeleitet (Anfragen, Updates, Einladungen),
// einmalige Ereignisse (z. B. "Anfrage angenommen") landen im Verlauf. Gelesen-Status lokal.

const store = {
  get(k, def) { try { const v = localStorage.getItem(k); return v ? JSON.parse(v) : def; } catch { return def; } },
  set(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch {} },
};
const BELL = '<svg viewBox="0 0 24 24"><path d="M6 9a6 6 0 0 1 12 0c0 5 2 6.5 2 6.5H4S6 14 6 9"/><path d="M10 19a2 2 0 0 0 4 0"/></svg>';
const userKey = () => S.session.user?.id || 'gast';
const seenSet = () => new Set(store.get('nnn.seen.' + userKey(), []));
const dismissedSet = () => new Set(store.get('nnn.dismissed.' + userKey(), []));

function addEvent(ev) {
  const k = 'nnn.events.' + userKey();
  const list = store.get(k, []);
  if (list.some((e) => e.key === ev.key)) return;
  list.unshift({ ...ev, time: Math.floor(Date.now() / 1000) });
  store.set(k, list.slice(0, 40));
  toast(ev.title);
}

function buildNotifications() {
  const out = [];
  const me = S.session.user?.id;
  for (const u of S.friends.incoming) out.push({
    key: 'fr:' + u.id, kind: 'friend', time: u.since, user: u.username,
    title: `${u.username} möchte mit dir befreundet sein`,
    actions: [{ label: 'Annehmen', act: 'acceptFriend', data: { user: u.id }, primary: true }, { label: 'Ablehnen', act: 'removeFriend', data: { user: u.id } }],
  });
  for (const inst of S.instances) {
    const p = packFor(inst);
    if (p && p.latestVersion > inst.packVersion) out.push({
      key: `up:${p.id}:${p.latestVersion}`, kind: 'update', time: p.updatedAt,
      title: `Update für „${inst.name}“`, text: `Version ${p.latestVersion} ist verfügbar (du hast ${inst.packVersion}).`,
      actions: [{ label: 'Aktualisieren', act: 'sync', data: { id: inst.id }, primary: true }, { label: 'Ansehen', act: 'open', data: { id: inst.id } }],
    });
    else if (p && inst.dirty && ['owner', 'editor'].includes(p.role)) out.push({
      key: `dirty:${inst.id}:${inst.mods.length}:${inst.packVersion}`, kind: 'dirty', time: null,
      title: `„${inst.name}“ hat unveröffentlichte Änderungen`, text: 'Deine Freunde haben diese Mods noch nicht.',
      actions: [{ label: 'Veröffentlichen', act: 'push', data: { id: inst.id }, primary: true }],
    });
  }
  for (const p of S.packs) {
    if (p.owner.id === me || S.instances.some((i) => i.packId === p.id)) continue;
    out.push({
      key: 'inv:' + p.id, kind: 'invite', time: p.createdAt,
      title: `Du bist in „${p.name}“`, text: `Modpack von ${p.owner.username} · Minecraft ${p.minecraft} · ${LOADER_NAMES[p.loader]}`,
      actions: [{ label: 'Installieren', act: 'installPack', data: { pack: p.id }, primary: true }],
    });
  }
  const dismissed = dismissedSet();
  const events = store.get('nnn.events.' + userKey(), []).filter((e) => !dismissed.has(e.key)).map((e) => ({ ...e, history: true }));
  return { active: out, events };
}

function notifIcon(n) {
  if (n.user) return `<div class="notif-ico"><img src="https://mc-heads.net/avatar/${encodeURIComponent(n.user)}/64" alt=""></div>`;
  const map = { update: ['acc', ICON.sync], dirty: ['acc', ICON.upload], invite: ['blue', ICON.box], 'friend-accepted': ['good', ICON.check] };
  const [cls, ico] = map[n.kind] || ['', BELL];
  return `<div class="notif-ico ${cls}">${ico}</div>`;
}

function notifHtml(n, seen) {
  const actions = (n.actions || []).map((a) =>
    `<button class="btn small ${a.primary ? 'primary' : ''}" data-act="${a.act}" ${Object.entries(a.data).map(([k, v]) => `data-${k}="${esc(v)}"`).join(' ')} data-notif="1">${esc(a.label)}</button>`).join('');
  return `<div class="notif ${seen.has(n.key) ? '' : 'unseen'}">${notifIcon(n)}
    <div class="notif-body"><div class="notif-title">${esc(n.title)}</div>
      ${n.text ? `<div class="notif-text">${esc(n.text)}</div>` : ''}
      ${n.time ? `<div class="notif-time">${esc(fmtDate(n.time))}</div>` : ''}
      ${actions ? `<div class="notif-actions">${actions}</div>` : ''}</div>
    ${n.history ? `<button class="notif-x" title="Entfernen" data-dismiss="${esc(n.key)}">${ICON.x}</button>` : ''}</div>`;
}

let lastUnseen = 0;
function renderNotifs() {
  const { active, events } = buildNotifications();
  const seen = seenSet();
  const unseen = [...active, ...events].filter((n) => !seen.has(n.key)).length;
  const badge = $('#notifBadge');
  badge.textContent = unseen > 9 ? '9+' : unseen;
  badge.classList.toggle('hidden', !unseen);
  const btn = $('#notifBtn');
  if (unseen > lastUnseen) { btn.classList.remove('ring'); void btn.offsetWidth; btn.classList.add('ring'); }
  lastUnseen = unseen;

  const panel = $('#notifPanel');
  if (panel.classList.contains('hidden')) return;
  panel.innerHTML = `<div class="notif-head"><h3>Benachrichtigungen</h3>
      ${unseen ? '<button data-notif-read>Alle gelesen</button>' : ''}
      ${events.length ? '<button data-notif-clear>Verlauf leeren</button>' : ''}</div>
    <div class="notif-list">
      ${!loggedIn() ? `<div class="notif-empty">${BELL}<div>Melde dich unter „Freunde“ an, um Updates und Anfragen zu sehen.</div></div>` : ''}
      ${loggedIn() && !active.length && !events.length ? `<div class="notif-empty">${BELL}<div>Alles erledigt – keine neuen Benachrichtigungen.</div></div>` : ''}
      ${active.map((n) => notifHtml(n, seen)).join('')}
      ${events.length ? `<div class="notif-sep">Verlauf</div>${events.map((n) => notifHtml(n, seen)).join('')}` : ''}
    </div>`;
}

function markAllSeen() {
  const { active, events } = buildNotifications();
  const seen = seenSet();
  for (const n of [...active, ...events]) seen.add(n.key);
  store.set('nnn.seen.' + userKey(), [...seen].slice(-300));
}

function toggleNotifs(open) {
  const panel = $('#notifPanel');
  const willOpen = open ?? panel.classList.contains('hidden');
  panel.classList.toggle('hidden', !willOpen);
  $('#notifBtn').classList.toggle('open', willOpen);
  if (willOpen) {
    renderNotifs();
    // Beim Öffnen frisch laden; als gelesen gilt alles erst beim Schließen
    Promise.all([refreshPacks(), refreshFriends()]).then(() => renderNotifs());
  } else {
    markAllSeen();
    renderNotifs();
  }
}

// ================= Rendern =================

function go(view, extra = {}) {
  Object.assign(S, { view }, extra);
  render();
  main.scrollTop = 0;
}

function render() {
  renderSidebar();
  document.querySelectorAll('#nav button').forEach((b) =>
    b.classList.toggle('active', b.dataset.view === S.view || (S.view === 'instance' && b.dataset.view === 'library')));
  const views = { library: viewLibrary, instance: viewInstance, discover: viewDiscover, friends: viewFriends, settings: viewSettings };
  main.innerHTML = (views[S.view] || viewLibrary)();
  afterRender();
}

const officialMode = () => S.settings.launchMode !== 'direct';

function renderSidebar() {
  const acc = officialMode() ? null : S.accounts.accounts.find((a) => a.id === S.accounts.selected) || S.accounts.accounts[0];
  $('#accountName').textContent = officialMode() ? 'Minecraft Launcher' : acc ? acc.name : 'Kein Konto';
  $('#accountType').textContent = officialMode() ? 'Login über Mojang' : acc ? (acc.type === 'msa' ? 'Microsoft' : 'Offline') : 'Minecraft-Konto hinzufügen';
  const head = $('#accountHead');
  if (acc) { head.src = `https://mc-heads.net/avatar/${encodeURIComponent(acc.name)}/64`; head.style.display = ''; }
  else head.style.display = 'none';
  $('#accountBtn').style.paddingLeft = acc ? '' : '14px';
  const n = S.friends.incoming.length;
  const badge = $('#friendBadge');
  badge.textContent = n;
  badge.classList.toggle('hidden', !n);
  const st = $('#serverStatus');
  if (!loggedIn()) st.innerHTML = `<span class="dot"></span>Nicht angemeldet`;
  else st.innerHTML = `<span class="dot ${S.serverOk === false ? 'off' : 'on'}"></span>${esc(S.session.user?.username)}${S.serverOk === false ? ' · offline' : ''}`;
  renderNotifs();
}

function progressHtml(job) {
  const p = S.progress[job];
  if (!p) return `<div class="progress hidden" data-job="${esc(job)}"></div>`;
  const indet = !p.total || p.total <= 1;
  const pct = indet ? 0 : Math.round((p.done / p.total) * 100);
  return `<div class="progress" data-job="${esc(job)}">
    <div class="label"><span>${esc(p.stage)}${p.detail ? ' · ' + esc(p.detail) : ''}</span><span>${indet ? '' : `${p.done}/${p.total}`}</span></div>
    <div class="bar ${indet ? 'indet' : ''}"><div style="width:${pct}%"></div></div></div>`;
}

function updateProgressEls(job) {
  document.querySelectorAll(`[data-job="${CSS.escape(job)}"]`).forEach((el) => {
    const tmp = document.createElement('div');
    tmp.innerHTML = progressHtml(job);
    el.replaceWith(tmp.firstElementChild);
  });
}

const isBusy = (id) => ['launch-', 'sync-', 'mods-'].some((k) => S.progress[k + id]);

function playButton(inst, big = false) {
  const cls = big ? 'big' : 'small';
  if (S.running.has(inst.id)) return `<button class="btn danger ${cls}" data-act="kill" data-id="${esc(inst.id)}">${ICON.stop}Beenden</button>`;
  if (S.progress['launch-' + inst.id]) return `<button class="btn ${cls}" disabled><span class="spinner"></span>Startet…</button>`;
  return `<button class="btn primary ${cls}" data-act="play" data-id="${esc(inst.id)}" ${isBusy(inst.id) ? 'disabled' : ''}>${ICON.play}Spielen</button>`;
}

function instTags(inst) {
  let t = `<span class="tag">${esc(inst.minecraft)}</span><span class="tag">${esc(LOADER_NAMES[inst.loader])}</span>`;
  if (inst.packId) t += `<span class="tag blue">${ICON.link}Geteilt</span>`;
  if (hasUpdate(inst)) t += `<span class="tag acc">${ICON.sync}Update</span>`;
  else if (inst.dirty && ['owner', 'editor'].includes(inst.packRole)) t += `<span class="tag warn">Unveröffentlicht</span>`;
  return t;
}

// ---------- Bibliothek ----------

function viewLibrary() {
  const notInstalled = S.packs.filter((p) => !S.instances.some((i) => i.packId === p.id));
  // Zuletzt gespielte (oder neueste) Instanz groß oben
  const hero = [...S.instances].sort((a, b) => (b.lastPlayed || 0) - (a.lastPlayed || 0) || (b.createdAt || 0) - (a.createdAt || 0))[0];
  const rest = S.instances.filter((i) => i !== hero);
  const modCount = (i) => `${i.mods.length} Mod${i.mods.length === 1 ? '' : 's'}`;
  return `
  <div class="page-head"><h1>Deine <em>Packs</em></h1>
    <button class="btn" data-act="join">${ICON.join}Beitreten</button>
    <button class="btn primary" data-act="newInstance">${ICON.plus}Neue Instanz</button></div>
  ${hero ? `
  <div class="hero" style="--hero-c:${colorFor(hero.name)}">
    ${instIcon(hero.name)}
    <div class="grow"><div class="kicker">${hero.lastPlayed ? 'Weiterspielen' : 'Neu'}</div>
      <h2 class="title">${esc(hero.name)}</h2>
      <div class="meta">${instTags(hero)}<span class="muted small">${modCount(hero)}${hero.lastPlayed ? ' · zuletzt ' + esc(fmtDate(hero.lastPlayed)) : ''}</span></div>
      ${progressHtml('launch-' + hero.id)}${progressHtml('sync-' + hero.id)}</div>
    <div class="row">
      <button class="btn" data-act="open" data-id="${esc(hero.id)}">Öffnen</button>
      ${hasUpdate(hero) ? `<button class="btn" data-act="sync" data-id="${esc(hero.id)}">${ICON.sync}Aktualisieren</button>` : ''}
      ${playButton(hero, true)}</div>
  </div>` : ''}
  ${rest.length ? `<h2>Alle Instanzen</h2><div class="cards">${rest.map((i) => `
    <div class="card inst" data-act="open" data-id="${esc(i.id)}">
      <div class="inst-banner" style="background:linear-gradient(135deg, color-mix(in srgb, ${colorFor(i.name)} 40%, transparent), transparent 80%)">${instIcon(i.name)}</div>
      <div class="inst-body">
        <div style="min-width:0"><h3>${esc(i.name)}</h3>
          <div class="muted small">${modCount(i)}${i.lastPlayed ? ' · zuletzt ' + esc(fmtDate(i.lastPlayed)) : ''}</div></div>
        <div class="row" style="flex-wrap:wrap;gap:6px">${instTags(i)}</div>
        ${progressHtml('launch-' + i.id)}${progressHtml('sync-' + i.id)}
        <div class="inst-bottom">${playButton(i)}
          ${hasUpdate(i) ? `<button class="btn small" data-act="sync" data-id="${esc(i.id)}">${ICON.sync}Aktualisieren</button>` : ''}</div>
      </div>
    </div>`).join('')}</div>` : ''}
  ${S.instances.length ? '' : `<div class="empty">${ICON.box}<div>Noch keine Instanzen.</div><div class="small">Leg eine neue an oder tritt einem Modpack deiner Freunde bei.</div></div>`}
  ${notInstalled.length ? `<h2>Geteilte Packs – noch nicht installiert</h2><div class="list">${notInstalled.map((p) => `
    <div class="item">${instIcon(p.name)}<div class="main"><div class="title">${esc(p.name)}</div>
      <div class="sub">${esc(p.minecraft)} · ${esc(LOADER_NAMES[p.loader])} · von ${esc(p.owner.username)} · Version ${p.latestVersion}</div></div>
      ${progressHtml('install-' + p.id)}
      <button class="btn primary small" data-act="installPack" data-pack="${esc(p.id)}">Installieren</button></div>`).join('')}</div>` : ''}`;
}

// ---------- Instanz ----------

function viewInstance() {
  const inst = getInst(S.instanceId);
  if (!inst) return viewLibrary();
  const pack = packFor(inst);
  const canEdit = !inst.packId || ['owner', 'editor'].includes(inst.packRole);
  let banner = '';
  if (hasUpdate(inst)) banner = `<div class="banner acc">${ICON.sync}<div class="grow"><b>Update verfügbar:</b> Version ${pack.latestVersion} (du hast ${inst.packVersion})</div>
      <button class="btn primary small" data-act="sync" data-id="${esc(inst.id)}">Jetzt aktualisieren</button></div>`;
  else if (inst.dirty && canEdit && inst.packId) banner = `<div class="banner warn"><div class="grow">Du hast Änderungen, die deine Freunde noch nicht haben.</div>
      <button class="btn primary small" data-act="push" data-id="${esc(inst.id)}">${ICON.upload}Update veröffentlichen</button></div>`;
  else if (inst.dirty && !canEdit) banner = `<div class="banner warn"><div class="grow">Lokale Änderungen werden beim nächsten Pack-Update überschrieben.</div></div>`;

  const tabs = [['mods', 'Mods'], ['pack', 'Teilen & Freunde'], ['log', 'Log'], ['settings', 'Einstellungen']];
  return `
  <button class="back" data-act="nav" data-view="library">${ICON.back}Bibliothek</button>
  <div class="detail-head" style="--hero-c:${colorFor(inst.name)}">${instIcon(inst.name)}
    <div style="flex:1;min-width:0"><h1>${esc(inst.name)}</h1><div class="row" style="gap:6px;flex-wrap:wrap">${instTags(inst)}
      <span class="muted small">${inst.loaderVersion ? esc(LOADER_NAMES[inst.loader]) + ' ' + esc(inst.loaderVersion) : ''}</span></div></div>
    ${playButton(inst, true)}</div>
  ${progressHtml('launch-' + inst.id)}${progressHtml('sync-' + inst.id)}${progressHtml('mods-' + inst.id)}
  ${banner}
  <div class="tabs">${tabs.map(([k, l]) => `<button data-act="tab" data-tab="${k}" class="${S.tab === k ? 'active' : ''}">${l}</button>`).join('')}</div>
  <div id="tabBody">${({ mods: tabMods, pack: tabPack, log: tabLog, settings: tabInstSettings }[S.tab] || tabMods)(inst, pack, canEdit)}</div>`;
}

function tabMods(inst, pack, canEdit) {
  const mods = [...inst.mods].sort((a, b) => a.name.localeCompare(b.name));
  return `
  <div class="toolbar"><div class="muted grow">${mods.length} Mod${mods.length === 1 ? '' : 's'} im Pack</div>
    <button class="btn" data-act="openFolder" data-id="${esc(inst.id)}">${ICON.folder}Ordner</button>
    ${inst.loader !== 'vanilla' ? `<button class="btn primary" data-act="addMods" data-id="${esc(inst.id)}">${ICON.plus}Mods hinzufügen</button>` : ''}</div>
  ${inst.loader === 'vanilla' ? `<div class="empty">Vanilla-Instanzen haben keine Mods. Wähle in den Einstellungen einen Mod-Loader.</div>` : ''}
  <div class="list">${mods.map((m) => `
    <div class="item">${m.iconUrl ? `<img src="${esc(m.iconUrl)}" alt="" loading="lazy">` : '<div class="ph"></div>'}
      <div class="main"><div class="title">${esc(m.name)}${m.dependency ? ' <span class="tag">Abhängigkeit</span>' : ''}</div>
        <div class="sub">${esc(m.versionName || m.fileName)} · ${m.source === 'curseforge' ? 'CurseForge' : 'Modrinth'}</div></div>
      ${canEdit ? `<button class="btn icon danger" title="Entfernen" data-act="removeMod" data-id="${esc(inst.id)}" data-file="${esc(m.fileName)}">${ICON.trash}</button>` : ''}
    </div>`).join('')}</div>
  <div id="localMods"></div>`;
}

function tabPack(inst, pack) {
  if (!loggedIn()) return `<div class="card"><h3>Mit Freunden teilen</h3><p class="muted">Melde dich bei NNN Launcher an, um dieses Modpack zu teilen und Updates zu synchronisieren.</p>
    <button class="btn primary" data-act="nav" data-view="friends">Anmelden</button></div>`;
  if (!inst.packId) return `<div class="card"><h3>Dieses Modpack teilen</h3>
    <p class="muted">Lade das Pack auf deinen NNN-Server. Danach kannst du Freunde einladen oder einen Link verschicken. Wenn du später Mods änderst, klickst du auf „Update veröffentlichen“ – alle anderen bekommen es mit einem Klick.</p>
    <button class="btn primary" data-act="publish" data-id="${esc(inst.id)}">${ICON.upload}Pack teilen</button></div>`;
  if (!pack) return `<div class="card"><p class="muted">Pack-Infos werden geladen… Falls das so bleibt, wurde das Pack gelöscht oder du wurdest entfernt.</p>
    <button class="btn danger" data-act="unlink" data-id="${esc(inst.id)}">Verknüpfung lösen</button></div>`;
  const canEdit = ['owner', 'editor'].includes(pack.role);
  return `
  <div class="split"><div>
    <div class="card section"><div class="row"><div class="grow"><h3>${esc(pack.name)}</h3>
      <div class="muted small">Version ${pack.latestVersion} · Besitzer ${esc(pack.owner.username)} · deine Rolle: ${({ owner: 'Besitzer', editor: 'Editor', member: 'Mitglied' })[pack.role]}</div></div>
      ${canEdit ? `<button class="btn" data-act="share" data-pack="${esc(pack.id)}">${ICON.link}Link erstellen</button>
        <button class="btn primary" data-act="push" data-id="${esc(inst.id)}">${ICON.upload}Update veröffentlichen</button>` : ''}</div></div>
    <h2>Versionen</h2><div id="versions" class="list"><div class="center"><span class="spinner"></span></div></div>
  </div><div>
    <h2 style="margin-top:0">Mitglieder</h2><div id="members" class="list"><div class="center"><span class="spinner"></span></div></div>
    ${canEdit ? `<h2>Freund einladen</h2><div class="row"><select id="inviteSel" class="grow">
      ${S.friends.friends.length ? S.friends.friends.map((f) => `<option value="${f.id}">${esc(f.username)}</option>`).join('') : '<option value="">Noch keine Freunde</option>'}
      </select><button class="btn" data-act="invite" data-pack="${esc(pack.id)}">Einladen</button></div>` : ''}
    <h2>Gefahrenzone</h2>
    ${pack.role === 'owner' ? `<button class="btn danger" data-act="deletePack" data-id="${esc(inst.id)}" data-pack="${esc(pack.id)}">${ICON.trash}Pack auf dem Server löschen</button>`
      : `<button class="btn danger" data-act="leavePack" data-id="${esc(inst.id)}" data-pack="${esc(pack.id)}">Pack verlassen</button>`}
  </div></div>`;
}

function tabLog(inst) {
  const lines = S.logs[inst.id] || [];
  return `<div class="toolbar"><div class="muted grow">${S.running.has(inst.id) ? 'Spiel läuft' : 'Letzte Ausgabe dieser Sitzung'}</div>
    <button class="btn small" data-act="clearLog" data-id="${esc(inst.id)}">Leeren</button></div>
    <div class="log" id="logBox">${lines.map(logLine).join('')}</div>`;
}
function logLine(l) {
  const cls = /\b(ERROR|FATAL|Exception)\b/.test(l) ? 'e' : /\bWARN\b/.test(l) ? 'w' : '';
  return `<div class="${cls}">${esc(l)}</div>`;
}

function tabInstSettings(inst, pack, canEdit) {
  return `<div style="max-width:560px">
    <label class="field"><span>Name</span><input type="text" id="setName" value="${esc(inst.name)}"></label>
    <label class="field"><span>Arbeitsspeicher (MB, leer = Standard ${S.settings.memoryMb})</span>
      <input type="number" id="setMem" min="1024" step="512" value="${inst.memoryMb || ''}"></label>
    ${canEdit ? `<div class="grid2">
      <label class="field"><span>Minecraft</span><select id="setMc">${mcOptions(inst.minecraft)}</select></label>
      <label class="field"><span>Loader</span><select id="setLoader">${Object.entries(LOADER_NAMES).map(([k, v]) => `<option value="${k}" ${k === inst.loader ? 'selected' : ''}>${v}</option>`).join('')}</select></label></div>
      <label class="field" id="setLvWrap"><span>Loader-Version</span><select id="setLv"><option>${esc(inst.loaderVersion || '')}</option></select></label>
      <p class="muted small">Achtung: Beim Wechsel der Minecraft-Version oder des Loaders passen installierte Mods evtl. nicht mehr.</p>`
      : `<p class="muted small">Version und Loader legt der Besitzer des Packs fest.</p>`}
    <div class="row"><button class="btn primary" data-act="saveInst" data-id="${esc(inst.id)}">Speichern</button>
      <button class="btn" data-act="openFolder" data-id="${esc(inst.id)}">${ICON.folder}Ordner öffnen</button>
      <span class="grow"></span>
      <button class="btn danger" data-act="deleteInst" data-id="${esc(inst.id)}">${ICON.trash}Instanz löschen</button></div></div>`;
}

function mcOptions(selected) {
  const list = S.mcVersions.length ? S.mcVersions : [{ id: selected }];
  const ids = list.map((v) => v.id);
  if (selected && !ids.includes(selected)) ids.unshift(selected);
  return ids.map((id) => `<option ${id === selected ? 'selected' : ''}>${esc(id)}</option>`).join('');
}

// ---------- Mods finden ----------

function viewDiscover() {
  const d = S.discover;
  const moddable = S.instances.filter((i) => i.loader !== 'vanilla');
  if (!d.target || !getInst(d.target)) d.target = moddable[0]?.id || null;
  return `
  <div class="page-head"><h1>Mods <em>finden</em></h1></div>
  <div class="toolbar">
    <div class="seg"><button data-act="source" data-source="modrinth" class="${d.source === 'modrinth' ? 'active' : ''}">Modrinth</button>
      <button data-act="source" data-source="curseforge" class="${d.source === 'curseforge' ? 'active' : ''}">CurseForge</button></div>
    <input type="search" id="q" placeholder="Mods durchsuchen…" value="${esc(d.query)}">
    <select id="target" title="Instanz">${moddable.length ? moddable.map((i) => `<option value="${esc(i.id)}" ${i.id === d.target ? 'selected' : ''}>${esc(i.name)} (${esc(i.minecraft)} ${esc(LOADER_NAMES[i.loader])})</option>`).join('') : '<option value="">Keine Instanz mit Mod-Loader</option>'}</select>
  </div>
  ${d.target ? progressHtml('mods-' + d.target) : ''}
  <div id="results">${resultsHtml()}</div>`;
}

function resultsHtml() {
  const d = S.discover;
  if (d.source === 'curseforge' && !loggedIn()) return `<div class="empty">CurseForge läuft über deinen NNN-Server. Bitte erst <a href="#" data-act="nav" data-view="friends">anmelden</a>.</div>`;
  if (d.error) return `<div class="empty">${esc(d.error)}</div>`;
  if (d.loading && !d.results.length) return `<div class="center"><span class="spinner"></span></div>`;
  if (!d.results.length) return `<div class="empty">Nichts gefunden.</div>`;
  const inst = getInst(d.target);
  const installed = new Set((inst?.mods || []).map((m) => m.source + ':' + m.projectId));
  return `<div class="list">${d.results.map((r, idx) => `
    <div class="item big">${r.icon ? `<img src="${esc(r.icon)}" alt="" loading="lazy">` : '<div class="ph"></div>'}
      <div class="main"><div class="title">${esc(r.title)} <span class="muted small">von ${esc(r.author || '?')}</span></div>
        <div class="desc">${esc(r.description)}</div>
        <div class="sub">${fmtNum(r.downloads || 0)} Downloads${r.url ? ` · <a href="${esc(r.url)}" data-act="ext" data-url="${esc(r.url)}">Seite</a>` : ''}</div></div>
      ${installed.has(r.source + ':' + r.id) ? `<span class="tag acc">${ICON.check}Installiert</span>`
        : `<button class="btn primary small" data-act="install" data-idx="${idx}" ${inst ? '' : 'disabled'}>${ICON.plus}Installieren</button>`}
    </div>`).join('')}</div>
  ${d.results.length < d.total ? `<div class="center"><button class="btn" data-act="more">${d.loading ? '<span class="spinner"></span>' : 'Mehr laden'}</button></div>` : ''}`;
}

let searchTimer = null;
async function runSearch(append = false) {
  const d = S.discover;
  const inst = getInst(d.target);
  if (d.source === 'curseforge' && !loggedIn()) { $('#results') && ($('#results').innerHTML = resultsHtml()); return; }
  d.loading = true; d.error = null;
  if (!append) { d.offset = 0; d.results = []; }
  const token = (d._token = Symbol());
  if ($('#results')) $('#results').innerHTML = resultsHtml();
  try {
    const r = await sources[d.source].search(d.query, inst, d.offset);
    if (token !== d._token) return;
    d.results = append ? d.results.concat(r.items) : r.items;
    d.total = r.total;
    d.offset = d.results.length;
  } catch (e) {
    if (token === d._token) d.error = e.message;
  }
  d.loading = false;
  if ($('#results')) $('#results').innerHTML = resultsHtml();
}

// ---------- Freunde ----------

function viewFriends() {
  if (!loggedIn()) return viewAuth();
  const f = S.friends;
  const person = (u, actions) => `<div class="item"><img src="https://mc-heads.net/avatar/${encodeURIComponent(u.username)}/64" alt="" onerror="this.style.visibility='hidden'">
    <div class="main"><div class="title">${esc(u.username)}</div></div>${actions}</div>`;
  return `
  <div class="page-head"><h1><em>Freunde</em></h1></div>
  <div class="split"><div>
    ${f.incoming.length ? `<h2 style="margin-top:0">Anfragen</h2><div class="list section">${f.incoming.map((u) => person(u,
      `<button class="btn primary small" data-act="acceptFriend" data-user="${u.id}">${ICON.check}Annehmen</button>
       <button class="btn small" data-act="removeFriend" data-user="${u.id}">Ablehnen</button>`)).join('')}</div>` : ''}
    <h2 style="margin-top:0">Deine Freunde (${f.friends.length})</h2>
    ${f.friends.length ? `<div class="list">${f.friends.map((u) => person(u,
      `<button class="btn icon danger" title="Entfernen" data-act="removeFriend" data-user="${u.id}" data-name="${esc(u.username)}">${ICON.x}</button>`)).join('')}</div>`
      : `<div class="empty">${ICON.users}<div>Noch keine Freunde. Füge jemanden über den Benutzernamen hinzu.</div></div>`}
    ${f.outgoing.length ? `<h2>Gesendet</h2><div class="list">${f.outgoing.map((u) => person(u,
      `<span class="muted small">wartet…</span><button class="btn small" data-act="removeFriend" data-user="${u.id}">Zurückziehen</button>`)).join('')}</div>` : ''}
  </div><div>
    <div class="card"><h3>Freund hinzufügen</h3><p class="muted small">NNN-Benutzername eingeben</p>
      <div class="row"><input type="text" id="friendName" placeholder="Benutzername" list="userSuggest" autocomplete="off"><datalist id="userSuggest"></datalist>
      <button class="btn primary" data-act="addFriend">${ICON.plus}</button></div></div>
    <div class="card" style="margin-top:14px"><h3>Angemeldet als ${esc(S.session.user?.username)}</h3>
      <p class="muted small">Deine Freunde finden dich unter diesem Namen.</p>
      <button class="btn small" data-act="logout">Abmelden</button></div>
  </div></div>`;
}

function viewAuth() {
  return `<div class="page-head"><h1>Bei <em>NNN</em> anmelden</h1></div>
  <div class="card" style="max-width:420px">
    <p class="muted">Mit einem NNN-Konto kannst du Freunde hinzufügen, Modpacks teilen und Updates synchronisieren. Das ist unabhängig von deinem Minecraft-Konto.</p>
    <label class="field"><span>Benutzername</span><input type="text" id="authUser" autocomplete="username"></label>
    <label class="field"><span>Passwort</span><input type="password" id="authPass" autocomplete="current-password"></label>
    <div class="row"><button class="btn primary" data-act="login">Anmelden</button><button class="btn" data-act="register">Konto erstellen</button></div>
    <p class="muted small">Server: ${esc(S.settings.serverUrl)} (änderbar in den Einstellungen)</p>
  </div>`;
}

// ---------- Einstellungen ----------

function viewSettings() {
  const s = S.settings;
  return `<div class="page-head"><h1><em>Einstellungen</em></h1></div>
  <div style="max-width:640px">
    <h2 style="margin-top:0">Spielstart</h2>
    <label class="field"><span>Minecraft starten über</span><select id="sMode">
      <option value="official" ${s.launchMode !== 'direct' ? 'selected' : ''}>Offiziellen Minecraft Launcher (empfohlen, wie CurseForge)</option>
      <option value="direct" ${s.launchMode === 'direct' ? 'selected' : ''}>NNN Launcher direkt (eigener Microsoft-Login nötig)</option></select></label>
    <p class="muted small">Beim offiziellen Launcher legt NNN Launcher ein Profil „NNN: &lt;Name&gt;“ an und öffnet den Launcher von Mojang. Anmeldung und Spielstart laufen dort. Fehlt er auf dem PC, lädt NNN Launcher ihn einmalig von Mojang herunter.</p>
    <label class="field"><span>Pfad zum Minecraft Launcher (optional)</span><input type="text" id="sLauncher" value="${esc(s.officialLauncherPath)}" placeholder="automatisch"></label>
    <div id="directOnly" class="${s.launchMode === 'direct' ? '' : 'hidden'}">
    <h2>Minecraft-Konten</h2>
    <div class="list section">${S.accounts.accounts.map((a) => `
      <div class="item"><img src="https://mc-heads.net/avatar/${encodeURIComponent(a.name)}/64" alt="">
        <div class="main"><div class="title">${esc(a.name)}</div><div class="sub">${a.type === 'msa' ? 'Microsoft-Konto' : 'Offline (nur Einzelspieler / Offline-Server)'}</div></div>
        ${a.id === S.accounts.selected ? `<span class="tag acc">Aktiv</span>` : `<button class="btn small" data-act="selectAcc" data-acc="${esc(a.id)}">Verwenden</button>`}
        <button class="btn icon danger" data-act="removeAcc" data-acc="${esc(a.id)}" title="Entfernen">${ICON.trash}</button></div>`).join('')}
      <div class="row"><button class="btn primary" data-act="msLogin">Mit Microsoft anmelden</button><button class="btn" data-act="offlineAcc">Offline-Konto</button></div></div>

    <h2>Microsoft-Login</h2>
    <label class="field"><span>Azure-Client-ID</span><input type="text" id="sClient" value="${esc(s.msClientId)}" placeholder="00000000-0000-0000-0000-000000000000"></label>
    <p class="muted small">Eigene App unter portal.azure.com registrieren (Konten: „Nur persönliche Microsoft-Konten“, Öffentliche Clientflows: an) und bei Mojang für die Minecraft-API freischalten lassen.</p>
    </div>

    <h2>NNN-Server</h2>
    <label class="field"><span>Server-Adresse</span><input type="text" id="sServer" value="${esc(s.serverUrl)}"></label>

    <h2>Java & Spiel</h2>
    <label class="field"><span>Arbeitsspeicher (MB)</span><input type="number" id="sMem" min="1024" step="512" value="${s.memoryMb}"></label>
    <label class="field"><span>Zusätzliche JVM-Argumente</span><input type="text" id="sJvm" value="${esc(s.jvmArgs)}" placeholder="-XX:+UseG1GC"></label>
    <label class="check"><input type="checkbox" id="sSnap" ${s.showSnapshots ? 'checked' : ''}>Snapshots anzeigen</label>
    <label class="check"><input type="checkbox" id="sClose" ${s.closeOnLaunch ? 'checked' : ''}>Launcher beim Spielstart minimieren</label>
    <button class="btn primary" data-act="saveSettings">Speichern</button>
    <p class="muted small" style="margin-top:20px">NNN Launcher ${esc(S.appVersion || '')} · Java wird automatisch passend zur Minecraft-Version geladen.</p>
  </div>`;
}

// ---------- Nach dem Rendern ----------

function afterRender() {
  if (S.view === 'instance') {
    const inst = getInst(S.instanceId);
    if (!inst) return;
    if (S.tab === 'log') { const b = $('#logBox'); if (b) b.scrollTop = b.scrollHeight; }
    if (S.tab === 'mods') loadLocalMods(inst);
    if (S.tab === 'pack' && inst.packId && packFor(inst)) loadPackDetails(inst);
    if (S.tab === 'settings' && $('#setLoader')) {
      const refresh = () => loadLoaderVersions($('#setLoader').value, $('#setMc').value, $('#setLv'), inst.loader === $('#setLoader').value && inst.minecraft === $('#setMc').value ? inst.loaderVersion : null);
      $('#setLoader').onchange = refresh; $('#setMc').onchange = refresh;
      refresh();
    }
  }
  if (S.view === 'discover') {
    const q = $('#q');
    q.oninput = () => { S.discover.query = q.value; clearTimeout(searchTimer); searchTimer = setTimeout(() => runSearch(), 300); };
    const t = $('#target');
    t.onchange = () => { S.discover.target = t.value; render(); runSearch(); };
    if (!S.discover.results.length && !S.discover.loading && !S.discover.error) runSearch();
  }
  if (S.view === 'friends' && loggedIn()) {
    const inp = $('#friendName');
    let tmr;
    inp.oninput = () => {
      clearTimeout(tmr);
      tmr = setTimeout(async () => {
        if (inp.value.trim().length < 2) return;
        try {
          const r = await api('/users/search?q=' + encodeURIComponent(inp.value.trim()));
          $('#userSuggest').innerHTML = r.users.map((u) => `<option value="${esc(u.username)}">`).join('');
        } catch {}
      }, 250);
    };
    inp.onkeydown = (e) => { if (e.key === 'Enter') actions.addFriend(); };
  }
  if (S.view === 'friends' && !loggedIn()) {
    $('#authPass').onkeydown = (e) => { if (e.key === 'Enter') actions.login(); };
  }
}

async function loadLocalMods(inst) {
  try {
    const list = await call('instances.localMods', { id: inst.id });
    const el = $('#localMods');
    if (!el || !list.length) return;
    el.innerHTML = `<h2>Eigene Dateien im mods-Ordner</h2><p class="muted small">Diese Mods hast du selbst hineinkopiert. Sie werden nicht mit dem Pack geteilt.</p>
      <div class="list">${list.map((m) => `<div class="item"><div class="ph"></div><div class="main"><div class="title">${esc(m.fileName)}</div>
      <div class="sub">${(m.size / 1024 / 1024).toFixed(1).replace('.', ',')} MB</div></div></div>`).join('')}</div>`;
  } catch {}
}

async function loadPackDetails(inst) {
  const pack = packFor(inst);
  try {
    const [detail, versions] = await Promise.all([api(`/packs/${pack.id}`), api(`/packs/${pack.id}/versions`)]);
    const me = S.session.user?.id;
    const isOwner = pack.role === 'owner';
    const canEdit = ['owner', 'editor'].includes(pack.role);
    const roleName = { owner: 'Besitzer', editor: 'Editor', member: 'Mitglied' };
    const m = $('#members');
    if (m) m.innerHTML = detail.pack.members.map((u) => `<div class="item"><img src="https://mc-heads.net/avatar/${encodeURIComponent(u.username)}/64" alt="">
      <div class="main"><div class="title">${esc(u.username)}${u.id === me ? ' (du)' : ''}</div><div class="sub">${roleName[u.role]}</div></div>
      ${isOwner && u.role !== 'owner' ? `<button class="btn small" data-act="role" data-pack="${esc(pack.id)}" data-user="${u.id}" data-role="${u.role === 'editor' ? 'member' : 'editor'}" title="Editoren dürfen Updates veröffentlichen">${u.role === 'editor' ? 'Zu Mitglied' : 'Zu Editor'}</button>` : ''}
      ${canEdit && u.role !== 'owner' && u.id !== me ? `<button class="btn icon danger" data-act="kick" data-pack="${esc(pack.id)}" data-user="${u.id}" title="Entfernen">${ICON.x}</button>` : ''}
      </div>`).join('');
    const v = $('#versions');
    if (v) v.innerHTML = versions.versions.map((x) => `<div class="item"><div class="main">
      <div class="title">Version ${x.version}${x.version === inst.packVersion ? ' <span class="tag acc">installiert</span>' : ''}</div>
      <div class="sub">${esc(x.author)} · ${esc(fmtDate(x.createdAt))}</div>${x.changelog ? `<div class="desc selectable">${esc(x.changelog)}</div>` : ''}</div></div>`).join('');
  } catch (e) { fail(e); }
}

async function loadLoaderVersions(loader, mc, select, current) {
  const wrap = $('#setLvWrap') || $('#nLvWrap');
  if (loader === 'vanilla') { select.innerHTML = ''; wrap && wrap.classList.add('hidden'); return; }
  wrap && wrap.classList.remove('hidden');
  select.innerHTML = '<option>Lädt…</option>';
  select.disabled = true;
  try {
    const list = await call('loader.versions', { loader, minecraft: mc });
    if (!list.length) { select.innerHTML = '<option value="">Nicht verfügbar für diese Version</option>'; return; }
    const def = current || (list.find((x) => x.recommended) || list.find((x) => x.stable) || list[0]).version;
    select.innerHTML = list.slice(0, 80).map((x) => `<option value="${esc(x.version)}" ${x.version === def ? 'selected' : ''}>${esc(x.version)}${x.recommended ? ' (empfohlen)' : x.stable ? '' : ' (beta)'}</option>`).join('');
    select.disabled = false;
  } catch (e) {
    select.innerHTML = `<option value="">Fehler: ${esc(e.message)}</option>`;
  }
}

// ================= Aktionen =================

const actions = {
  nav: (d) => go(d.view),
  open: (d) => go('instance', { instanceId: d.id, tab: S.instanceId === d.id ? S.tab : 'mods' }),
  tab: (d) => { S.tab = d.tab; render(); },
  ext: (d) => call('shell.open', { url: d.url }),

  async play(d, btn) {
    S.progress['launch-' + d.id] = { stage: 'Vorbereiten', done: 0, total: 0 };
    S.logs[d.id] = [];
    render();
    try {
      const r = await call('game.launch', { id: d.id });
      if (r && r.mode === 'official') {
        toast(`Minecraft Launcher geöffnet – Profil „${r.profileName}“ ist ausgewählt. Dort auf „Spielen“ klicken.`);
        if (r.alreadyRunning) toast('Der Minecraft Launcher war schon offen. Falls das Profil fehlt: Launcher schließen und hier nochmal auf „Spielen“ klicken.', 'warn');
      }
    }
    catch (e) { fail(e); }
    delete S.progress['launch-' + d.id];
    render();
  },
  kill: (d) => call('game.kill', { id: d.id }).catch(fail),

  newInstance() {
    const m = modal(`<h2>Neue Instanz</h2>
      <label class="field"><span>Name</span><input type="text" id="nName" placeholder="z. B. Survival mit Freunden"></label>
      <div class="grid2"><label class="field"><span>Minecraft</span><select id="nMc">${mcOptions(S.mcVersions[0]?.id)}</select></label>
      <label class="field"><span>Loader</span><select id="nLoader">${Object.entries(LOADER_NAMES).map(([k, v]) => `<option value="${k}" ${k === 'fabric' ? 'selected' : ''}>${v}</option>`).join('')}</select></label></div>
      <label class="field" id="nLvWrap"><span>Loader-Version</span><select id="nLv"></select></label>
      <div class="actions"><button class="btn" data-close>Abbrechen</button><button class="btn primary" id="nOk">Erstellen</button></div>`);
    const refresh = () => loadLoaderVersions($('#nLoader', m.el).value, $('#nMc', m.el).value, $('#nLv', m.el));
    $('#nLoader', m.el).onchange = refresh; $('#nMc', m.el).onchange = refresh; refresh();
    $('#nOk', m.el).onclick = (e) => busy(e.currentTarget, async () => {
      const loader = $('#nLoader', m.el).value;
      const lv = $('#nLv', m.el).value;
      if (loader !== 'vanilla' && !lv) return toast('Bitte eine Loader-Version wählen', 'warn');
      try {
        const inst = await call('instances.create', { name: $('#nName', m.el).value || 'Neue Instanz', minecraft: $('#nMc', m.el).value, loader, loaderVersion: lv });
        upsertInstance(inst);
        m.close();
        go('instance', { instanceId: inst.id, tab: 'mods' });
      } catch (err) { fail(err); }
    });
  },

  addMods(d) { S.discover.target = d.id; S.discover.results = []; go('discover'); },

  async install(d, btn) {
    const r = S.discover.results[Number(d.idx)];
    const inst = getInst(S.discover.target);
    if (!r || !inst) return;
    await busy(btn, async () => { try { await installMod(r, inst); } catch (e) { fail(e); } });
    if ($('#results')) $('#results').innerHTML = resultsHtml();
  },
  source(d) { S.discover.source = d.source; render(); runSearch(); },
  more: () => runSearch(true),

  async removeMod(d) {
    try { upsertInstance(await call('instances.removeMod', { id: d.id, fileName: d.file })); render(); } catch (e) { fail(e); }
  },
  openFolder: (d) => call('instances.openFolder', { id: d.id }).catch(fail),
  clearLog(d) { S.logs[d.id] = []; render(); },

  async saveInst(d, btn) {
    await busy(btn, async () => {
      try {
        const p = { id: d.id, name: $('#setName').value, memoryMb: Number($('#setMem').value) || 0 };
        if ($('#setLoader')) {
          p.minecraft = $('#setMc').value; p.loader = $('#setLoader').value; p.loaderVersion = $('#setLv').value || null;
          if (p.loader !== 'vanilla' && !p.loaderVersion) throw new Error('Bitte eine Loader-Version wählen');
        }
        upsertInstance(await call('instances.update', p));
        toast('Gespeichert');
      } catch (e) { fail(e); }
    });
    render();
  },
  async deleteInst(d) {
    const inst = getInst(d.id);
    if (!(await confirmDialog('Instanz löschen?', `„${inst.name}“ wird mit allen Welten und Einstellungen von deinem PC gelöscht.`, 'Löschen', true))) return;
    try { await call('instances.delete', { id: d.id }); S.instances = S.instances.filter((i) => i.id !== d.id); go('library'); } catch (e) { fail(e); }
  },

  // ----- Packs -----

  async sync(d, btn) {
    const inst = getInst(d.id);
    const pack = packFor(inst);
    if (!pack) return;
    S.progress['sync-' + inst.id] = { stage: 'Update laden', done: 0, total: 0 };
    render();
    try {
      const v = await api(`/packs/${pack.id}/versions/latest`);
      upsertInstance(await call('pack.apply', { id: inst.id, packId: pack.id, version: v.version, manifest: v.manifest, role: pack.role }));
      toast(`„${pack.name}“ ist jetzt auf Version ${v.version}${v.changelog ? ': ' + v.changelog : ''}`);
    } catch (e) { fail(e); }
    delete S.progress['sync-' + inst.id];
    render();
  },

  async installPack(d) {
    const pack = S.packs.find((p) => p.id === d.pack);
    await joinAndInstall(pack);
  },

  publish(d) {
    const inst = getInst(d.id);
    const m = modal(`<h2>Pack teilen</h2>
      <label class="field"><span>Name</span><input type="text" id="pName" value="${esc(inst.name)}"></label>
      <label class="field"><span>Beschreibung (optional)</span><textarea id="pDesc" placeholder="Worum geht's?"></textarea></label>
      <label class="check"><input type="checkbox" id="pCfg" checked>Config-Dateien mitschicken (config, defaultconfigs, kubejs)</label>
      <div class="actions"><button class="btn" data-close>Abbrechen</button><button class="btn primary" id="pOk">${ICON.upload}Hochladen</button></div>`);
    $('#pOk', m.el).onclick = (e) => busy(e.currentTarget, async () => {
      try {
        const manifest = await call('pack.buildManifest', { id: inst.id, includeConfigs: $('#pCfg', m.el).checked });
        const { pack } = await api('/packs', { method: 'POST', body: { name: $('#pName', m.el).value, description: $('#pDesc', m.el).value, manifest, changelog: 'Erste Version' } });
        if ($('#pName', m.el).value.trim() && $('#pName', m.el).value.trim() !== inst.name)
          upsertInstance(await call('instances.update', { id: inst.id, name: pack.name }));
        upsertInstance(await call('pack.link', { id: inst.id, packId: pack.id, version: 1, role: 'owner' }));
        await refreshPacks();
        m.close();
        S.tab = 'pack';
        render();
        actions.share({ pack: pack.id });
      } catch (err) { fail(err); }
    });
  },

  push(d) {
    const inst = getInst(d.id);
    const added = inst.mods.length;
    const m = modal(`<h2>Update veröffentlichen</h2>
      <p class="muted">Alle Mitglieder von „${esc(inst.name)}“ bekommen das Update angezeigt und können es mit einem Klick laden. (${added} Mods)</p>
      <label class="field"><span>Was ist neu?</span><textarea id="uLog" placeholder="z. B. Create + Farmer's Delight dazu, JEI aktualisiert"></textarea></label>
      <label class="check"><input type="checkbox" id="uCfg" checked>Config-Dateien mitschicken</label>
      <div class="actions"><button class="btn" data-close>Abbrechen</button><button class="btn primary" id="uOk">${ICON.upload}Veröffentlichen</button></div>`);
    $('#uOk', m.el).onclick = (e) => busy(e.currentTarget, async () => {
      try {
        const manifest = await call('pack.buildManifest', { id: inst.id, includeConfigs: $('#uCfg', m.el).checked });
        const r = await api(`/packs/${inst.packId}/versions`, { method: 'POST', body: { baseVersion: inst.packVersion, manifest, changelog: $('#uLog', m.el).value } });
        upsertInstance(await call('pack.link', { id: inst.id, packId: inst.packId, version: r.version, role: inst.packRole }));
        await refreshPacks();
        m.close();
        toast(`Version ${r.version} veröffentlicht`);
        render();
      } catch (err) { fail(err); }
    });
  },

  share(d) {
    const m = modal(`<h2>Einladungslink</h2>
      <p class="muted">Schick den Link an Freunde (z. B. in Discord). Mit NNN Launcher installieren sie das Pack mit einem Klick und bekommen alle Updates.</p>
      <div class="grid2"><label class="field"><span>Gültig</span><select id="sDays"><option value="0">Unbegrenzt</option><option value="1">1 Tag</option><option value="7" selected>7 Tage</option><option value="30">30 Tage</option></select></label>
      <label class="field"><span>Max. Nutzungen</span><select id="sUses"><option value="0">Unbegrenzt</option><option value="1">1</option><option value="5">5</option><option value="10" selected>10</option></select></label></div>
      <div id="sOut"></div>
      <div class="actions"><button class="btn" data-close>Schließen</button><button class="btn primary" id="sOk">${ICON.link}Link erstellen</button></div>`, { wide: true });
    $('#sOk', m.el).onclick = (e) => busy(e.currentTarget, async () => {
      try {
        const r = await api(`/packs/${d.pack}/share`, { method: 'POST', body: { expiresInDays: Number($('#sDays', m.el).value), maxUses: Number($('#sUses', m.el).value) } });
        $('#sOut', m.el).innerHTML = `<div class="code-box">${esc(r.code)}</div>
          <div class="copy-row"><input type="text" readonly value="${esc(r.url)}"><button class="btn" data-copy="${esc(r.url)}">${ICON.copy}Link</button></div>
          <div class="copy-row"><input type="text" readonly value="${esc(r.code)}"><button class="btn" data-copy="${esc(r.code)}">${ICON.copy}Code</button></div>`;
        m.el.querySelectorAll('[data-copy]').forEach((b) => (b.onclick = () => copy(b.dataset.copy)));
        copy(r.url);
      } catch (err) { fail(err); }
    });
  },

  join(d) {
    if (!loggedIn()) { toast('Bitte zuerst bei NNN Launcher anmelden', 'warn'); return go('friends'); }
    const m = modal(`<h2>Modpack beitreten</h2>
      <label class="field"><span>Code oder Link</span><input type="text" id="jCode" placeholder="z. B. ZN3cfTbw" value="${esc(d.code || '')}"></label>
      <div id="jPrev"></div>
      <div class="actions"><button class="btn" data-close>Abbrechen</button><button class="btn primary" id="jOk">Weiter</button></div>`);
    let preview = null;
    const parse = () => { const v = $('#jCode', m.el).value.trim(); const mm = v.match(/([A-Za-z0-9]{6,16})\/?$/); return mm ? mm[1] : ''; };
    const showPreview = async () => {
      const code = parse();
      if (!code) return toast('Ungültiger Code', 'warn');
      try {
        preview = await api('/share/' + code);
        const p = preview.pack;
        $('#jPrev', m.el).innerHTML = `<div class="item">${instIcon(p.name)}<div class="main"><div class="title">${esc(p.name)}</div>
          <div class="sub">${esc(p.minecraft)} · ${esc(LOADER_NAMES[p.loader])} · von ${esc(p.owner)} · ${p.members} Mitglied(er)</div>
          ${p.description ? `<div class="desc">${esc(p.description)}</div>` : ''}</div></div>`;
        $('#jOk', m.el).textContent = 'Beitreten & installieren';
      } catch (err) { preview = null; fail(err); }
    };
    $('#jOk', m.el).onclick = (e) => busy(e.currentTarget, async () => {
      if (!preview) return showPreview();
      try {
        const { pack } = await api(`/share/${preview.code}/join`, { method: 'POST' });
        m.close();
        await refreshPacks();
        await joinAndInstall(S.packs.find((p) => p.id === pack.id) || pack);
      } catch (err) { fail(err); }
    });
    $('#jCode', m.el).onkeydown = (e) => { if (e.key === 'Enter') $('#jOk', m.el).click(); };
    if (d.code) showPreview();
  },

  async invite(d) {
    const id = $('#inviteSel').value;
    if (!id) return;
    try { await api(`/packs/${d.pack}/members`, { method: 'POST', body: { userId: Number(id) } }); toast('Eingeladen – das Pack erscheint jetzt in der Bibliothek deines Freundes'); render(); } catch (e) { fail(e); }
  },
  async kick(d) {
    if (!(await confirmDialog('Mitglied entfernen?', 'Die Person bekommt keine Updates mehr für dieses Pack.', 'Entfernen', true))) return;
    try { await api(`/packs/${d.pack}/members/${d.user}`, { method: 'DELETE' }); render(); } catch (e) { fail(e); }
  },
  async role(d) {
    try { await api(`/packs/${d.pack}/members/${d.user}`, { method: 'PUT', body: { role: d.role } }); render(); } catch (e) { fail(e); }
  },
  async leavePack(d) {
    if (!(await confirmDialog('Pack verlassen?', 'Die Instanz bleibt auf deinem PC, bekommt aber keine Updates mehr.', 'Verlassen', true))) return;
    try {
      await api(`/packs/${d.pack}/members/${S.session.user.id}`, { method: 'DELETE' });
      upsertInstance(await call('pack.unlink', { id: d.id }));
      await refreshPacks(); render();
    } catch (e) { fail(e); }
  },
  async deletePack(d) {
    if (!(await confirmDialog('Pack löschen?', 'Das Pack wird für alle Mitglieder vom Server gelöscht. Deine lokale Instanz bleibt erhalten.', 'Löschen', true))) return;
    try {
      await api(`/packs/${d.pack}`, { method: 'DELETE' });
      upsertInstance(await call('pack.unlink', { id: d.id }));
      await refreshPacks(); render();
    } catch (e) { fail(e); }
  },
  async unlink(d) { upsertInstance(await call('pack.unlink', { id: d.id })); render(); },

  // ----- Freunde & Login -----

  async login(d, btn) { await authAction('/auth/login', btn); },
  async register(d, btn) { await authAction('/auth/register', btn); },
  async logout() {
    try { await api('/auth/logout', { method: 'POST' }); } catch {}
    await setSession(null, null);
    S.packs = []; S.friends = { friends: [], incoming: [], outgoing: [] };
    render();
  },
  async addFriend() {
    const inp = $('#friendName');
    const name = inp.value.trim();
    if (!name) return;
    try {
      const r = await api('/friends/request', { method: 'POST', body: { username: name } });
      toast(r.status === 'accepted' ? `Du bist jetzt mit ${name} befreundet` : `Anfrage an ${name} gesendet`);
      await refreshFriends(); render();
    } catch (e) { fail(e); }
  },
  async acceptFriend(d) {
    try { await api('/friends/accept', { method: 'POST', body: { userId: Number(d.user) } }); await refreshFriends(); render(); } catch (e) { fail(e); }
  },
  async removeFriend(d) {
    if (d.name && !(await confirmDialog('Freund entfernen?', `${d.name} wird aus deiner Freundesliste entfernt.`, 'Entfernen', true))) return;
    try { await api('/friends/remove', { method: 'POST', body: { userId: Number(d.user) } }); await refreshFriends(); render(); } catch (e) { fail(e); }
  },

  // ----- Konten & Einstellungen -----

  async selectAcc(d) { S.accounts = await call('accounts.select', { id: d.acc }); render(); },
  async removeAcc(d) {
    if (!(await confirmDialog('Konto entfernen?', 'Das Konto wird aus NNN Launcher entfernt.', 'Entfernen', true))) return;
    S.accounts = await call('accounts.remove', { id: d.acc }); render();
  },
  offlineAcc() {
    const m = modal(`<h2>Offline-Konto</h2><p class="muted">Funktioniert nur im Einzelspieler und auf Servern mit online-mode=false.</p>
      <label class="field"><span>Spielername</span><input type="text" id="oName" maxlength="16"></label>
      <div class="actions"><button class="btn" data-close>Abbrechen</button><button class="btn primary" id="oOk">Hinzufügen</button></div>`);
    $('#oOk', m.el).onclick = async () => {
      try { S.accounts = await call('accounts.addOffline', { name: $('#oName', m.el).value }); m.close(); render(); } catch (e) { fail(e); }
    };
  },
  msLogin() {
    if (!S.settings.msClientId) { toast('Bitte zuerst eine Azure-Client-ID eintragen und speichern', 'warn'); $('#sClient')?.focus(); return; }
    let closed = false;
    const m = modal(`<h2>Mit Microsoft anmelden</h2><div id="msBody" class="center"><span class="spinner"></span></div>
      <div class="actions"><button class="btn" data-close>Abbrechen</button></div>`,
      { onClose: () => { closed = true; call('job.cancel', { job: 'ms-login' }); } });
    msCodeHandler = (c) => {
      $('#msBody', m.el).outerHTML = `<div><p class="muted">Öffne die Seite und gib diesen Code ein:</p><div class="code-box">${esc(c.userCode)}</div>
        <div class="row"><button class="btn primary" id="msOpen">Seite öffnen</button><button class="btn" id="msCopy">${ICON.copy}Code kopieren</button></div>
        <p class="muted small">Warte auf Anmeldung… <span class="spinner"></span></p></div>`;
      $('#msOpen', m.el).onclick = () => { copy(c.userCode); call('shell.open', { url: c.verificationUri }); };
      $('#msCopy', m.el).onclick = () => copy(c.userCode);
    };
    call('accounts.msLogin').then((acc) => {
      S.accounts = acc; if (!closed) m.close(); toast('Microsoft-Konto hinzugefügt'); render();
    }).catch((e) => { if (!closed) { m.close(); fail(e); } });
  },
  async saveSettings(d, btn) {
    await busy(btn, async () => {
      try {
        const before = S.settings.serverUrl;
        S.settings = await call('settings.set', {
          ...S.settings, launchMode: $('#sMode').value, officialLauncherPath: $('#sLauncher').value.trim(), msClientId: $('#sClient').value.trim(), serverUrl: $('#sServer').value.trim(),
          memoryMb: Number($('#sMem').value) || 4096, jvmArgs: $('#sJvm').value, showSnapshots: $('#sSnap').checked, closeOnLaunch: $('#sClose').checked,
        });
        if (before !== S.settings.serverUrl) { await setSession(null, null); S.serverOk = null; }
        S.mcVersions = await call('mc.versions', { snapshots: S.settings.showSnapshots }).catch(() => S.mcVersions);
        toast('Einstellungen gespeichert');
      } catch (e) { fail(e); }
    });
    render();
  },
};

let msCodeHandler = null;

async function authAction(path, btn) {
  const username = $('#authUser').value.trim();
  const password = $('#authPass').value;
  await busy(btn, async () => {
    try {
      const r = await api(path, { method: 'POST', body: { username, password } });
      await setSession(r.token, r.user);
      await Promise.all([refreshPacks(), refreshFriends()]);
      toast(`Willkommen, ${r.user.username}!`);
    } catch (e) { fail(e); }
  });
  render();
}

async function joinAndInstall(pack) {
  const job = 'install-' + pack.id;
  S.progress[job] = { stage: 'Pack laden', done: 0, total: 0 };
  render();
  try {
    const v = await api(`/packs/${pack.id}/versions/latest`);
    const inst = await call('pack.apply', { packId: pack.id, version: v.version, manifest: v.manifest, role: pack.role || 'member', name: pack.name });
    upsertInstance(inst);
    toast(`„${pack.name}“ installiert`);
    delete S.progress[job];
    go('instance', { instanceId: inst.id, tab: 'mods' });
  } catch (e) {
    delete S.progress[job];
    fail(e);
    render();
  }
}

// ================= Events =================

document.addEventListener('click', (e) => {
  const el = e.target.closest('[data-act]');
  if (!el) return;
  if (el.tagName === 'A') e.preventDefault();
  const fn = actions[el.dataset.act];
  if (fn) Promise.resolve(fn(el.dataset, el)).catch(fail);
});
$('#nav').addEventListener('click', (e) => {
  const b = e.target.closest('button[data-view]');
  if (b) go(b.dataset.view);
});
$('#accountBtn').onclick = () => go('settings');

$('#notifBtn').addEventListener('click', (e) => { e.stopPropagation(); toggleNotifs(); });
$('#notifPanel').addEventListener('click', (e) => {
  e.stopPropagation();
  const d = e.target.closest('[data-dismiss]');
  if (d) {
    const set = dismissedSet(); set.add(d.dataset.dismiss);
    store.set('nnn.dismissed.' + userKey(), [...set].slice(-300));
    return renderNotifs();
  }
  if (e.target.closest('[data-notif-read]')) { markAllSeen(); return renderNotifs(); }
  if (e.target.closest('[data-notif-clear]')) {
    const set = dismissedSet();
    for (const ev of store.get('nnn.events.' + userKey(), [])) set.add(ev.key);
    store.set('nnn.dismissed.' + userKey(), [...set].slice(-300));
    return renderNotifs();
  }
  // Aktionsknöpfe (Annehmen, Aktualisieren, …) laufen über die normalen Aktionen
  const a = e.target.closest('[data-act]');
  if (a) {
    const fn = actions[a.dataset.act];
    // Navigation oder Dialog: Fenster schließen
    if (['open', 'push', 'installPack', 'sync'].includes(a.dataset.act)) toggleNotifs(false);
    if (fn) Promise.resolve(fn(a.dataset, a)).then(() => renderNotifs()).catch(fail);
  }
});
document.addEventListener('click', (e) => {
  if (!$('#notifPanel').classList.contains('hidden') && !e.target.closest('.notif-wrap') && !e.target.closest('.modal-back')) toggleNotifs(false);
});
document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && !$('#notifPanel').classList.contains('hidden')) toggleNotifs(false); });
document.addEventListener('change', (e) => {
  // Kontenbereich sofort ein-/ausblenden, wenn der Startmodus wechselt
  if (e.target.id === 'sMode') $('#directOnly').classList.toggle('hidden', e.target.value !== 'direct');
});

// Fortschritt: Bridge meldet { job, stage, done, total, detail }
bridge.on('progress', (p) => {
  const job = p.job;
  // sync-<id> von neuer Instanz (Beitreten) wird auf den install-Balken gespiegelt
  S.progress[job] = p;
  if (job.startsWith('sync-')) {
    const inst = getInst(job.slice(5));
    const packId = inst?.packId;
    Object.keys(S.progress).filter((k) => k.startsWith('install-') && (!packId || k === 'install-' + packId)).forEach((k) => { S.progress[k] = p; updateProgressEls(k); });
  }
  updateProgressEls(job);
});
bridge.on('progress-done', ({ job }) => {
  if (job.startsWith('launch-')) return; // Start-Status bleibt, bis game.launch fertig ist
  delete S.progress[job];
  updateProgressEls(job);
});
bridge.on('game-log', ({ id, line }) => {
  const arr = (S.logs[id] ||= []);
  arr.push(line);
  if (arr.length > 3000) arr.splice(0, arr.length - 3000);
  if (S.view === 'instance' && S.instanceId === id && S.tab === 'log') {
    const box = $('#logBox');
    if (box) {
      const stick = box.scrollTop + box.clientHeight >= box.scrollHeight - 30;
      box.insertAdjacentHTML('beforeend', logLine(line));
      while (box.childElementCount > 3000) box.firstElementChild.remove();
      if (stick) box.scrollTop = box.scrollHeight;
    }
  }
});
bridge.on('game-started', ({ id }) => { S.running.add(id); render(); });
bridge.on('game-exit', ({ id, code }) => {
  S.running.delete(id);
  if (code !== 0) toast(`Minecraft wurde mit Fehlercode ${code} beendet – siehe Log`, 'error');
  render();
});
bridge.on('ms-device-code', (c) => msCodeHandler && msCodeHandler(c));
bridge.on('deep-link', ({ code }) => actions.join({ code }));

// ================= Start =================

(async function init() {
  try {
    const [settings, session, accounts] = await Promise.all([call('settings.get'), call('session.get'), call('accounts.list')]);
    S.settings = settings; S.session = session || { token: null, user: null }; S.accounts = accounts;
    S.running = new Set(await call('game.running').catch(() => []));
    render();
    call('mc.versions', { snapshots: !!settings.showSnapshots }).then((v) => { S.mcVersions = v; }).catch(() => {});
    await refreshAll();
    if (loggedIn()) api('/me').catch(() => {});
    const info = await call('app.ready');
    S.appVersion = info?.version;
  } catch (e) {
    main.innerHTML = `<div class="empty">Start fehlgeschlagen: ${esc(e.message)}</div>`;
  }
  // Alle 30 s nach Pack-Updates und Freundschaftsanfragen schauen
  setInterval(async () => {
    if (!loggedIn()) return;
    const before = S.friends.incoming.length;
    await Promise.all([refreshPacks(), refreshFriends()]);
    if (S.friends.incoming.length > before) toast('Neue Freundschaftsanfrage');
    if (S.view === 'library' || (S.view === 'instance' && S.tab !== 'settings' && S.tab !== 'log')) render();
    else renderSidebar();
  }, 30000);
})();
