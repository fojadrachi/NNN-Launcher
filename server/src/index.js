// NNN Launcher Sync-Server (Cloudflare Worker + D1)
// Accounts, Freunde, Modpacks mit Versionen, Share-Links und CurseForge-Proxy.

const SESSION_DAYS = 60;
const MAX_MANIFEST_BYTES = 1_500_000;
const LOADERS = ['vanilla', 'fabric', 'quilt', 'forge', 'neoforge'];
const CF_BASE = 'https://api.curseforge.com';

export default {
  async fetch(request, env) {
    if (request.method === 'OPTIONS') return new Response(null, { status: 204, headers: cors() });
    let res;
    try {
      res = await route(request, env);
    } catch (err) {
      if (err instanceof HttpError) res = json({ error: err.message }, err.status);
      else {
        console.error(err);
        res = json({ error: 'Interner Serverfehler' }, 500);
      }
    }
    // Auch Fehlerantworten brauchen CORS, sonst sieht der Launcher die Meldung nicht
    res = new Response(res.body, res);
    for (const [k, v] of Object.entries(cors())) res.headers.set(k, v);
    return res;
  },
};

class HttpError extends Error {
  constructor(status, message) { super(message); this.status = status; }
}

function cors() {
  return {
    'Access-Control-Allow-Origin': '*',
    'Access-Control-Allow-Methods': 'GET, POST, PUT, DELETE, OPTIONS',
    'Access-Control-Allow-Headers': 'Authorization, Content-Type',
    'Access-Control-Max-Age': '86400',
  };
}

function json(data, status = 200) {
  return new Response(JSON.stringify(data), {
    status,
    headers: { 'Content-Type': 'application/json; charset=utf-8' },
  });
}

const now = () => Math.floor(Date.now() / 1000);

async function body(request) {
  try { return await request.json(); } catch { throw new HttpError(400, 'Ungültiges JSON'); }
}

// ---------- Routing ----------

async function route(request, env) {
  const url = new URL(request.url);
  const p = url.pathname.replace(/\/+$/, '') || '/';
  const m = request.method;
  let match;

  if (p === '/' && m === 'GET') return json({ name: 'NNN Launcher', ok: true });

  // Öffentliche Share-Seite (zum Verschicken in Discord & Co.)
  if ((match = p.match(/^\/j\/([A-Za-z0-9]{6,16})$/)) && m === 'GET') return sharePage(env, match[1], url);

  if (p === '/auth/register' && m === 'POST') return register(request, env);
  if (p === '/auth/login' && m === 'POST') return login(request, env);
  if ((match = p.match(/^\/share\/([A-Za-z0-9]{6,16})$/)) && m === 'GET') return sharePreview(env, match[1]);

  // Ab hier nur mit Login
  const user = await authenticate(request, env);

  if (p === '/auth/logout' && m === 'POST') return logout(request, env);
  if (p === '/me' && m === 'GET') return json({ user: publicUser(user) });
  if (p === '/users/search' && m === 'GET') return searchUsers(env, user, url.searchParams.get('q') || '');

  if (p === '/friends' && m === 'GET') return listFriends(env, user);
  if (p === '/friends/request' && m === 'POST') return friendRequest(request, env, user);
  if (p === '/friends/accept' && m === 'POST') return friendAccept(request, env, user);
  if (p === '/friends/remove' && m === 'POST') return friendRemove(request, env, user);

  if (p === '/packs' && m === 'GET') return listPacks(env, user);
  if (p === '/packs' && m === 'POST') return createPack(request, env, user);
  if ((match = p.match(/^\/packs\/([a-z0-9]{10})$/))) {
    if (m === 'GET') return getPack(env, user, match[1]);
    if (m === 'PUT') return updatePack(request, env, user, match[1]);
    if (m === 'DELETE') return deletePack(env, user, match[1]);
  }
  if ((match = p.match(/^\/packs\/([a-z0-9]{10})\/versions$/))) {
    if (m === 'GET') return listVersions(env, user, match[1]);
    if (m === 'POST') return pushVersion(request, env, user, match[1]);
  }
  if ((match = p.match(/^\/packs\/([a-z0-9]{10})\/versions\/(\d+|latest)$/)) && m === 'GET')
    return getVersion(env, user, match[1], match[2]);
  if ((match = p.match(/^\/packs\/([a-z0-9]{10})\/members$/)) && m === 'POST')
    return addMember(request, env, user, match[1]);
  if ((match = p.match(/^\/packs\/([a-z0-9]{10})\/members\/(\d+)$/))) {
    if (m === 'DELETE') return removeMember(env, user, match[1], Number(match[2]));
    if (m === 'PUT') return setMemberRole(request, env, user, match[1], Number(match[2]));
  }
  if ((match = p.match(/^\/packs\/([a-z0-9]{10})\/share$/)) && m === 'POST')
    return createShare(request, env, user, match[1], url);
  if ((match = p.match(/^\/share\/([A-Za-z0-9]{6,16})\/join$/)) && m === 'POST')
    return joinShare(env, user, match[1]);

  if (p.startsWith('/cf/')) return curseforgeProxy(request, env, url);

  throw new HttpError(404, 'Nicht gefunden');
}

// ---------- Krypto ----------

function hex(buf) {
  return [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, '0')).join('');
}

function randomHex(bytes) {
  return hex(crypto.getRandomValues(new Uint8Array(bytes)));
}

function randomCode(len, alphabet) {
  const out = [];
  const rnd = crypto.getRandomValues(new Uint8Array(len * 2));
  for (let i = 0; out.length < len && i < rnd.length; i++) {
    // Rejection sampling gegen Modulo-Bias
    if (rnd[i] < 256 - (256 % alphabet.length)) out.push(alphabet[rnd[i] % alphabet.length]);
  }
  while (out.length < len) out.push(alphabet[0]);
  return out.join('');
}

async function sha256(text) {
  return hex(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text)));
}

async function hashPassword(password, saltHex) {
  const key = await crypto.subtle.importKey('raw', new TextEncoder().encode(password), 'PBKDF2', false, ['deriveBits']);
  const salt = new Uint8Array(saltHex.match(/../g).map((h) => parseInt(h, 16)));
  const bits = await crypto.subtle.deriveBits({ name: 'PBKDF2', hash: 'SHA-256', salt, iterations: 100000 }, key, 256);
  return hex(bits);
}

function safeEqual(a, b) {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

// ---------- Auth ----------

const publicUser = (u) => ({ id: u.id, username: u.username });

function validateCredentials(data) {
  const username = String(data.username || '').trim();
  const password = String(data.password || '');
  if (!/^[A-Za-z0-9_]{3,20}$/.test(username))
    throw new HttpError(400, 'Benutzername: 3–20 Zeichen, nur Buchstaben, Zahlen und _');
  if (password.length < 8 || password.length > 200) throw new HttpError(400, 'Passwort: mindestens 8 Zeichen');
  return { username, password };
}

async function createSession(env, userId) {
  const token = randomHex(32);
  const t = now();
  await env.DB.prepare('INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)')
    .bind(await sha256(token), userId, t, t + SESSION_DAYS * 86400).run();
  return token;
}

async function register(request, env) {
  const { username, password } = validateCredentials(await body(request));
  const exists = await env.DB.prepare('SELECT 1 FROM users WHERE username = ?').bind(username).first();
  if (exists) throw new HttpError(409, 'Benutzername ist schon vergeben');
  const salt = randomHex(16);
  const res = await env.DB.prepare('INSERT INTO users (username, pw_hash, pw_salt, created_at) VALUES (?, ?, ?, ?)')
    .bind(username, await hashPassword(password, salt), salt, now()).run();
  const id = res.meta.last_row_id;
  return json({ token: await createSession(env, id), user: { id, username } }, 201);
}

async function login(request, env) {
  const data = await body(request);
  const username = String(data.username || '').trim();
  const password = String(data.password || '');
  const u = await env.DB.prepare('SELECT * FROM users WHERE username = ?').bind(username).first();
  if (!u || !safeEqual(await hashPassword(password, u.pw_salt), u.pw_hash))
    throw new HttpError(401, 'Benutzername oder Passwort falsch');
  // Abgelaufene Sessions des Nutzers aufräumen
  await env.DB.prepare('DELETE FROM sessions WHERE user_id = ? AND expires_at < ?').bind(u.id, now()).run();
  return json({ token: await createSession(env, u.id), user: publicUser(u) });
}

function bearer(request) {
  const h = request.headers.get('Authorization') || '';
  return h.startsWith('Bearer ') ? h.slice(7).trim() : '';
}

async function authenticate(request, env) {
  const token = bearer(request);
  if (!token) throw new HttpError(401, 'Nicht angemeldet');
  const row = await env.DB.prepare(
    'SELECT u.id, u.username FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.token_hash = ? AND s.expires_at > ?'
  ).bind(await sha256(token), now()).first();
  if (!row) throw new HttpError(401, 'Sitzung abgelaufen, bitte neu anmelden');
  return row;
}

async function logout(request, env) {
  await env.DB.prepare('DELETE FROM sessions WHERE token_hash = ?').bind(await sha256(bearer(request))).run();
  return json({ ok: true });
}

async function searchUsers(env, user, q) {
  q = q.trim();
  if (q.length < 2) return json({ users: [] });
  const { results } = await env.DB.prepare(
    "SELECT id, username FROM users WHERE username LIKE ? ESCAPE '\\' AND id != ? ORDER BY length(username) LIMIT 10"
  ).bind(q.replace(/[\\%_]/g, (c) => '\\' + c) + '%', user.id).all();
  return json({ users: results });
}

// ---------- Freunde ----------

const pair = (x, y) => (x < y ? [x, y] : [y, x]);

async function findUser(env, data) {
  let u = null;
  if (data.userId) u = await env.DB.prepare('SELECT id, username FROM users WHERE id = ?').bind(Number(data.userId)).first();
  else if (data.username) u = await env.DB.prepare('SELECT id, username FROM users WHERE username = ?').bind(String(data.username).trim()).first();
  if (!u) throw new HttpError(404, 'Nutzer nicht gefunden');
  return u;
}

async function areFriends(env, a, b) {
  const [x, y] = pair(a, b);
  const row = await env.DB.prepare("SELECT 1 FROM friendships WHERE user_a = ? AND user_b = ? AND status = 'accepted'").bind(x, y).first();
  return !!row;
}

async function listFriends(env, user) {
  const { results } = await env.DB.prepare(`
    SELECT u.id, u.username, f.status, f.requester, f.created_at
    FROM friendships f
    JOIN users u ON u.id = CASE WHEN f.user_a = ?1 THEN f.user_b ELSE f.user_a END
    WHERE f.user_a = ?1 OR f.user_b = ?1
    ORDER BY u.username COLLATE NOCASE`).bind(user.id).all();
  const friends = [], incoming = [], outgoing = [];
  for (const r of results) {
    const entry = { id: r.id, username: r.username, since: r.created_at };
    if (r.status === 'accepted') friends.push(entry);
    else if (r.requester === user.id) outgoing.push(entry);
    else incoming.push(entry);
  }
  return json({ friends, incoming, outgoing });
}

async function friendRequest(request, env, user) {
  const target = await findUser(env, await body(request));
  if (target.id === user.id) throw new HttpError(400, 'Du kannst dich nicht selbst hinzufügen');
  const [a, b] = pair(user.id, target.id);
  const existing = await env.DB.prepare('SELECT * FROM friendships WHERE user_a = ? AND user_b = ?').bind(a, b).first();
  if (existing) {
    if (existing.status === 'accepted') return json({ status: 'accepted' });
    if (existing.requester !== user.id) {
      // Gegenseitige Anfrage -> direkt annehmen
      await env.DB.prepare("UPDATE friendships SET status = 'accepted' WHERE user_a = ? AND user_b = ?").bind(a, b).run();
      return json({ status: 'accepted' });
    }
    return json({ status: 'pending' });
  }
  await env.DB.prepare("INSERT INTO friendships (user_a, user_b, requester, status, created_at) VALUES (?, ?, ?, 'pending', ?)")
    .bind(a, b, user.id, now()).run();
  return json({ status: 'pending' }, 201);
}

async function friendAccept(request, env, user) {
  const target = await findUser(env, await body(request));
  const [a, b] = pair(user.id, target.id);
  const res = await env.DB.prepare(
    "UPDATE friendships SET status = 'accepted' WHERE user_a = ? AND user_b = ? AND status = 'pending' AND requester != ?"
  ).bind(a, b, user.id).run();
  if (!res.meta.changes) throw new HttpError(404, 'Keine offene Anfrage von diesem Nutzer');
  return json({ status: 'accepted' });
}

async function friendRemove(request, env, user) {
  const target = await findUser(env, await body(request));
  const [a, b] = pair(user.id, target.id);
  await env.DB.prepare('DELETE FROM friendships WHERE user_a = ? AND user_b = ?').bind(a, b).run();
  return json({ ok: true });
}

// ---------- Packs ----------

async function membership(env, packId, userId) {
  return env.DB.prepare('SELECT role FROM pack_members WHERE pack_id = ? AND user_id = ?').bind(packId, userId).first();
}

async function requirePack(env, packId, userId, roles = ['owner', 'editor', 'member']) {
  const pack = await env.DB.prepare('SELECT * FROM packs WHERE id = ?').bind(packId).first();
  if (!pack) throw new HttpError(404, 'Modpack nicht gefunden');
  const mem = await membership(env, packId, userId);
  if (!mem) throw new HttpError(403, 'Du bist kein Mitglied dieses Modpacks');
  if (!roles.includes(mem.role)) throw new HttpError(403, 'Dafür fehlen dir die Rechte');
  return { pack, role: mem.role };
}

function validateManifest(manifest) {
  if (!manifest || typeof manifest !== 'object') throw new HttpError(400, 'Manifest fehlt');
  const raw = JSON.stringify(manifest);
  if (raw.length > MAX_MANIFEST_BYTES) throw new HttpError(413, 'Manifest zu groß (max. 1,5 MB inkl. Configs)');
  if (!/^\d+\.\d+(\.\d+)?$/.test(String(manifest.minecraft || ''))) throw new HttpError(400, 'Ungültige Minecraft-Version');
  if (!LOADERS.includes(manifest.loader)) throw new HttpError(400, 'Ungültiger Mod-Loader');
  if (!Array.isArray(manifest.mods)) throw new HttpError(400, 'mods muss eine Liste sein');
  for (const mod of manifest.mods) {
    if (!mod || typeof mod.fileName !== 'string' || !/^[^\\/:*?"<>|]+\.jar$/i.test(mod.fileName))
      throw new HttpError(400, 'Ungültiger Dateiname in mods');
    if (typeof mod.url !== 'string' || !/^https:\/\//.test(mod.url)) throw new HttpError(400, `Download-URL fehlt für ${mod.fileName}`);
  }
  for (const f of manifest.overrides || []) {
    const path = String(f?.path || '');
    if (!path || path.includes('..') || /^[\\/]|:/.test(path)) throw new HttpError(400, `Unsicherer Pfad: ${path}`);
  }
  return raw;
}

function packOut(row, role) {
  return {
    id: row.id, name: row.name, description: row.description,
    minecraft: row.mc_version, loader: row.loader,
    latestVersion: row.latest_version, owner: { id: row.owner_id, username: row.owner_name },
    role, createdAt: row.created_at, updatedAt: row.updated_at,
  };
}

async function listPacks(env, user) {
  const { results } = await env.DB.prepare(`
    SELECT p.*, m.role, u.username AS owner_name
    FROM pack_members m JOIN packs p ON p.id = m.pack_id JOIN users u ON u.id = p.owner_id
    WHERE m.user_id = ? ORDER BY p.updated_at DESC`).bind(user.id).all();
  return json({ packs: results.map((r) => packOut(r, r.role)) });
}

async function newPackId(env) {
  for (let i = 0; i < 5; i++) {
    const id = randomCode(10, 'abcdefghijkmnpqrstuvwxyz23456789');
    if (!(await env.DB.prepare('SELECT 1 FROM packs WHERE id = ?').bind(id).first())) return id;
  }
  throw new HttpError(500, 'Konnte keine Pack-ID erzeugen');
}

async function createPack(request, env, user) {
  const data = await body(request);
  const name = String(data.name || '').trim().slice(0, 64);
  if (!name) throw new HttpError(400, 'Name fehlt');
  const manifest = { formatVersion: 1, mods: [], overrides: [], ...data.manifest, name };
  const raw = validateManifest(manifest);
  const id = await newPackId(env);
  const t = now();
  await env.DB.batch([
    env.DB.prepare('INSERT INTO packs (id, owner_id, name, description, mc_version, loader, latest_version, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?)')
      .bind(id, user.id, name, String(data.description || '').slice(0, 500), manifest.minecraft, manifest.loader, t, t),
    env.DB.prepare("INSERT INTO pack_members (pack_id, user_id, role, joined_at) VALUES (?, ?, 'owner', ?)").bind(id, user.id, t),
    env.DB.prepare('INSERT INTO pack_versions (pack_id, version, manifest, changelog, author_id, created_at) VALUES (?, 1, ?, ?, ?, ?)')
      .bind(id, raw, String(data.changelog || 'Erste Version').slice(0, 2000), user.id, t),
  ]);
  return getPack(env, user, id, 201);
}

async function getPack(env, user, packId, status = 200) {
  const { role } = await requirePack(env, packId, user.id);
  const row = await env.DB.prepare('SELECT p.*, u.username AS owner_name FROM packs p JOIN users u ON u.id = p.owner_id WHERE p.id = ?').bind(packId).first();
  const { results: members } = await env.DB.prepare(
    'SELECT u.id, u.username, m.role, m.joined_at AS joinedAt FROM pack_members m JOIN users u ON u.id = m.user_id WHERE m.pack_id = ? ORDER BY m.joined_at'
  ).bind(packId).all();
  return json({ pack: { ...packOut(row, role), members } }, status);
}

async function updatePack(request, env, user, packId) {
  await requirePack(env, packId, user.id, ['owner', 'editor']);
  const data = await body(request);
  const name = data.name !== undefined ? String(data.name).trim().slice(0, 64) : null;
  if (name === '') throw new HttpError(400, 'Name darf nicht leer sein');
  await env.DB.prepare('UPDATE packs SET name = COALESCE(?, name), description = COALESCE(?, description), updated_at = ? WHERE id = ?')
    .bind(name, data.description !== undefined ? String(data.description).slice(0, 500) : null, now(), packId).run();
  return getPack(env, user, packId);
}

async function deletePack(env, user, packId) {
  await requirePack(env, packId, user.id, ['owner']);
  await env.DB.batch([
    env.DB.prepare('DELETE FROM share_links WHERE pack_id = ?').bind(packId),
    env.DB.prepare('DELETE FROM pack_versions WHERE pack_id = ?').bind(packId),
    env.DB.prepare('DELETE FROM pack_members WHERE pack_id = ?').bind(packId),
    env.DB.prepare('DELETE FROM packs WHERE id = ?').bind(packId),
  ]);
  return json({ ok: true });
}

async function listVersions(env, user, packId) {
  await requirePack(env, packId, user.id);
  const { results } = await env.DB.prepare(`
    SELECT v.version, v.changelog, v.created_at AS createdAt, u.username AS author
    FROM pack_versions v JOIN users u ON u.id = v.author_id
    WHERE v.pack_id = ? ORDER BY v.version DESC LIMIT 100`).bind(packId).all();
  return json({ versions: results });
}

async function getVersion(env, user, packId, which) {
  const { pack } = await requirePack(env, packId, user.id);
  const version = which === 'latest' ? pack.latest_version : Number(which);
  const row = await env.DB.prepare(`
    SELECT v.*, u.username AS author FROM pack_versions v JOIN users u ON u.id = v.author_id
    WHERE v.pack_id = ? AND v.version = ?`).bind(packId, version).first();
  if (!row) throw new HttpError(404, 'Version nicht gefunden');
  return json({
    version: row.version, changelog: row.changelog, author: row.author, createdAt: row.created_at,
    manifest: JSON.parse(row.manifest),
  });
}

async function pushVersion(request, env, user, packId) {
  const { pack } = await requirePack(env, packId, user.id, ['owner', 'editor']);
  const data = await body(request);
  // Schutz gegen gleichzeitiges Pushen: Client sagt, auf welcher Version er aufbaut
  if (data.baseVersion !== undefined && Number(data.baseVersion) !== pack.latest_version)
    throw new HttpError(409, `Jemand hat inzwischen Version ${pack.latest_version} hochgeladen. Bitte erst aktualisieren.`);
  const manifest = { formatVersion: 1, overrides: [], ...data.manifest, name: pack.name };
  const raw = validateManifest(manifest);
  const version = pack.latest_version + 1;
  const t = now();
  try {
    // Batch läuft als Transaktion; ein paralleler Push scheitert am Primärschlüssel (pack_id, version)
    await env.DB.batch([
      env.DB.prepare('INSERT INTO pack_versions (pack_id, version, manifest, changelog, author_id, created_at) VALUES (?, ?, ?, ?, ?, ?)')
        .bind(packId, version, raw, String(data.changelog || '').slice(0, 2000), user.id, t),
      env.DB.prepare('UPDATE packs SET latest_version = ?, mc_version = ?, loader = ?, updated_at = ? WHERE id = ?')
        .bind(version, manifest.minecraft, manifest.loader, t, packId),
    ]);
  } catch {
    throw new HttpError(409, 'Gleichzeitiges Update erkannt, bitte erneut versuchen');
  }
  return json({ version }, 201);
}

async function addMember(request, env, user, packId) {
  await requirePack(env, packId, user.id, ['owner', 'editor']);
  const target = await findUser(env, await body(request));
  if (!(await areFriends(env, user.id, target.id))) throw new HttpError(403, 'Du kannst nur Freunde direkt hinzufügen');
  await env.DB.prepare("INSERT OR IGNORE INTO pack_members (pack_id, user_id, role, joined_at) VALUES (?, ?, 'member', ?)")
    .bind(packId, target.id, now()).run();
  return getPack(env, user, packId);
}

async function removeMember(env, user, packId, targetId) {
  const { pack, role } = await requirePack(env, packId, user.id);
  if (targetId === pack.owner_id) throw new HttpError(400, 'Der Besitzer kann nicht entfernt werden');
  // Mitglieder dürfen sich selbst entfernen (Pack verlassen), Besitzer/Editor auch andere
  if (targetId !== user.id && role === 'member') throw new HttpError(403, 'Dafür fehlen dir die Rechte');
  await env.DB.prepare('DELETE FROM pack_members WHERE pack_id = ? AND user_id = ?').bind(packId, targetId).run();
  return json({ ok: true });
}

async function setMemberRole(request, env, user, packId, targetId) {
  const { pack } = await requirePack(env, packId, user.id, ['owner']);
  const roleNew = String((await body(request)).role || '');
  if (!['editor', 'member'].includes(roleNew)) throw new HttpError(400, 'Rolle muss editor oder member sein');
  if (targetId === pack.owner_id) throw new HttpError(400, 'Die Rolle des Besitzers kann nicht geändert werden');
  await env.DB.prepare('UPDATE pack_members SET role = ? WHERE pack_id = ? AND user_id = ?').bind(roleNew, packId, targetId).run();
  return getPack(env, user, packId);
}

// ---------- Share-Links ----------

async function createShare(request, env, user, packId, url) {
  await requirePack(env, packId, user.id, ['owner', 'editor']);
  let data = {};
  try { data = await request.json(); } catch {}
  const days = Math.min(Math.max(Number(data.expiresInDays) || 0, 0), 365);
  const maxUses = Math.max(Number(data.maxUses) || 0, 0) || null;
  const code = randomCode(8, 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789');
  const t = now();
  await env.DB.prepare('INSERT INTO share_links (code, pack_id, created_by, created_at, expires_at, max_uses) VALUES (?, ?, ?, ?, ?, ?)')
    .bind(code, packId, user.id, t, days ? t + days * 86400 : null, maxUses).run();
  return json({ code, url: `${url.origin}/j/${code}`, launcherUrl: `nnnlauncher://join/${code}` }, 201);
}

async function loadShare(env, code) {
  const row = await env.DB.prepare(`
    SELECT s.*, p.name, p.description, p.mc_version, p.loader, p.latest_version, u.username AS owner_name,
      (SELECT COUNT(*) FROM pack_members m WHERE m.pack_id = p.id) AS member_count
    FROM share_links s JOIN packs p ON p.id = s.pack_id JOIN users u ON u.id = p.owner_id
    WHERE s.code = ?`).bind(code).first();
  if (!row) throw new HttpError(404, 'Link ungültig');
  if (row.expires_at && row.expires_at < now()) throw new HttpError(410, 'Link ist abgelaufen');
  if (row.max_uses && row.uses >= row.max_uses) throw new HttpError(410, 'Link wurde schon zu oft benutzt');
  return row;
}

async function sharePreview(env, code) {
  const r = await loadShare(env, code);
  return json({
    code, pack: {
      id: r.pack_id, name: r.name, description: r.description, minecraft: r.mc_version, loader: r.loader,
      latestVersion: r.latest_version, owner: r.owner_name, members: r.member_count,
    },
  });
}

async function joinShare(env, user, code) {
  const r = await loadShare(env, code);
  const already = await membership(env, r.pack_id, user.id);
  if (!already) {
    await env.DB.batch([
      env.DB.prepare("INSERT INTO pack_members (pack_id, user_id, role, joined_at) VALUES (?, ?, 'member', ?)").bind(r.pack_id, user.id, now()),
      env.DB.prepare('UPDATE share_links SET uses = uses + 1 WHERE code = ?').bind(code),
    ]);
  }
  return getPack(env, user, r.pack_id);
}

const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

async function sharePage(env, code, url) {
  let r = null, err = null;
  try { r = await loadShare(env, code); } catch (e) { err = e.message; }
  const title = r ? `${r.name} – NNN Launcher` : 'NNN Launcher';
  const loaderName = r ? { vanilla: 'Vanilla', fabric: 'Fabric', quilt: 'Quilt', forge: 'Forge', neoforge: 'NeoForge' }[r.loader] : '';
  const html = `<!doctype html><html lang="de"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>${esc(title)}</title>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Space+Grotesk:wght@400;700&display=swap">
<meta property="og:title" content="${esc(r ? r.name : 'NNN Launcher')}">
<meta property="og:description" content="${esc(r ? `Modpack für Minecraft ${r.mc_version} (${loaderName}) von ${r.owner_name} – im NNN Launcher öffnen` : err)}">
<meta name="theme-color" content="#f5b43c">
<style>
:root{color-scheme:dark;--bg:#0e0c12;--card:#17141d;--fg:#ece8f2;--mut:#9a92a8;--acc:#f5b43c}
body{margin:0;min-height:100vh;display:grid;place-items:center;background:var(--bg);color:var(--fg);font:16px/1.5 "Space Grotesk",system-ui,Segoe UI,sans-serif;padding:16px;box-sizing:border-box}
.c{background:var(--card);border-radius:16px;padding:28px;max-width:420px;width:100%}
h1{margin:0 0 4px;font-size:24px}.m{color:var(--mut);font-size:14px}
a.b{display:block;margin-top:20px;background:var(--acc);color:#1a1305;text-align:center;padding:12px;border-radius:10px;font-weight:600;text-decoration:none}
code{background:#0003;padding:2px 6px;border-radius:6px}
</style></head><body><div class="c">
${r ? `<div class="m">Modpack-Einladung</div><h1>${esc(r.name)}</h1>
<div class="m">Minecraft ${esc(r.mc_version)} · ${esc(loaderName)} · von ${esc(r.owner_name)} · ${r.member_count} Mitglied(er)</div>
${r.description ? `<p>${esc(r.description)}</p>` : ''}
<a class="b" href="nnnlauncher://join/${esc(code)}">Im NNN Launcher öffnen</a>
<p class="m">Noch kein NNN Launcher? Installiere ihn und füge dort unter „Beitreten“ den Code <code>${esc(code)}</code> ein.</p>`
    : `<h1>Link ungültig</h1><p class="m">${esc(err)}</p>`}
</div></body></html>`;
  return new Response(html, { status: r ? 200 : 404, headers: { 'Content-Type': 'text/html; charset=utf-8' } });
}

// ---------- CurseForge-Proxy ----------
// Der API-Key bleibt als Secret im Worker und landet nie im Launcher.

async function curseforgeProxy(request, env, url) {
  if (!env.CF_API_KEY) throw new HttpError(503, 'CurseForge ist auf dem Server nicht eingerichtet (CF_API_KEY fehlt)');
  if (request.method !== 'GET' && request.method !== 'POST') throw new HttpError(405, 'Methode nicht erlaubt');
  const path = url.pathname.slice(3); // "/v1/..."
  if (!/^\/v1\/(mods|games|categories|minecraft)(\/|$)/.test(path)) throw new HttpError(400, 'Nicht erlaubter CurseForge-Pfad');
  // Beim Einfügen rutschen gern Leerzeichen, Zeilenumbrüche oder Anführungszeichen mit hinein
  const key = String(env.CF_API_KEY).trim().replace(/^["']|["']$/g, '').trim();
  const res = await fetch(CF_BASE + path + url.search, {
    method: request.method,
    headers: { 'x-api-key': key, Accept: 'application/json', 'Content-Type': 'application/json' },
    body: request.method === 'POST' ? await request.text() : undefined,
  });
  if (res.status === 401 || res.status === 403)
    throw new HttpError(502, 'CurseForge lehnt den API-Key ab – bitte CF_API_KEY prüfen');
  return new Response(res.body, { status: res.status, headers: { 'Content-Type': 'application/json' } });
}
