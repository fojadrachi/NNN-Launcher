using System.Diagnostics;
using System.Text.Json;
using System.Text.Json.Nodes;
using PackLink.Core;

namespace PackLink;

/// Verbindet die HTML-Oberfläche mit dem C#-Kern.
/// JS schickt {id, method, params}, C# antwortet mit {id, result} oder {id, error}.
/// Ereignisse (Fortschritt, Spiel-Log, Deep-Links) kommen als {event, data}.
class Bridge
{
    readonly MainForm _form;
    readonly Dictionary<string, CancellationTokenSource> _jobs = new();

    public Bridge(MainForm form) => _form = form;

    public void Handle(string json)
    {
        JsonNode? msg;
        try { msg = JsonNode.Parse(json); } catch { return; }
        var id = (string?)msg?["id"]?.ToString();
        var method = (string?)msg?["method"];
        var p = msg?["params"] as JsonObject ?? new JsonObject();
        if (id == null || method == null) return;

        _ = Task.Run(async () =>
        {
            try
            {
                var result = await Dispatch(method, p);
                _form.Post(new { id, result });
            }
            catch (Exception e)
            {
                var err = e is AggregateException ae ? ae.InnerException ?? e : e;
                _form.Post(new { id, error = err is OperationCanceledException ? "Abgebrochen" : err.Message });
            }
        });
    }

    static string S(JsonObject p, string key) => (string?)p[key] ?? throw new ArgumentException($"Parameter '{key}' fehlt");
    static string? SO(JsonObject p, string key) => p[key] is JsonValue v ? v.ToString() : null;

    IProgress<Core.Progress> ProgressFor(string job) =>
        new ThrottledProgress(pr => _form.Emit("progress", new { job, pr.Stage, pr.Done, pr.Total, pr.Detail }));

    CancellationToken StartJob(string job)
    {
        var cts = new CancellationTokenSource();
        lock (_jobs) { if (_jobs.Remove(job, out var old)) old.Cancel(); _jobs[job] = cts; }
        return cts.Token;
    }

    void EndJob(string job)
    {
        lock (_jobs) _jobs.Remove(job);
        _form.Emit("progress-done", new { job });
    }

    async Task<object?> Dispatch(string method, JsonObject p)
    {
        switch (method)
        {
            case "app.ready":
                _form.OnUiReady();
                return new { version = AppUpdater.CurrentText, root = AppPaths.Root, installed = AppUpdater.IsInstalled };

            // ---------- App-Updates ----------
            case "app.checkUpdate":
                return await AppUpdater.Check();
            case "app.installUpdate":
            {
                var ct = StartJob("app-update");
                try
                {
                    await AppUpdater.Install(ProgressFor("app-update"), ct);
                }
                finally { EndJob("app-update"); }
                // Kurz warten, damit die Oberfläche die Meldung noch zeigt, dann beenden (Setup ersetzt die Datei)
                _ = Task.Delay(800).ContinueWith(_ => _form.BeginInvoke(() => Application.Exit()));
                return true;
            }

            // ---------- Einstellungen ----------
            case "settings.get":
                return AppSettings.Current;
            case "settings.set":
            {
                var s = p.Deserialize<AppSettings>(Json.Options) ?? throw new ArgumentException("Ungültige Einstellungen");
                AppSettings.Save(s);
                return AppSettings.Current;
            }

            // ---------- PackLink-Server-Sitzung ----------
            case "session.get":
                return new { token = Accounts.Store.ServerToken, user = Accounts.Store.ServerUser };
            case "session.set":
                Accounts.Store.ServerToken = SO(p, "token");
                Accounts.Store.ServerUser = p["user"]?.DeepClone() as JsonObject;
                Accounts.Save();
                return true;

            // ---------- Minecraft-Konten ----------
            case "accounts.list":
                return Accounts.PublicList();
            case "accounts.addOffline":
                Accounts.AddOffline(S(p, "name"));
                return Accounts.PublicList();
            case "accounts.select":
                Accounts.Select(S(p, "id"));
                return Accounts.PublicList();
            case "accounts.remove":
                Accounts.Remove(S(p, "id"));
                return Accounts.PublicList();
            case "accounts.msLogin":
            {
                var ct = StartJob("ms-login");
                try
                {
                    var acc = await MicrosoftAuth.Login(AppSettings.Current.MsClientId,
                        code => _form.Emit("ms-device-code", new { code.UserCode, code.VerificationUri, code.Message }), ct);
                    Accounts.Upsert(acc);
                    return Accounts.PublicList();
                }
                finally { EndJob("ms-login"); }
            }

            // ---------- Versionen ----------
            case "mc.versions":
                return await Minecraft.ListVersions((bool?)p["snapshots"] ?? false);
            case "loader.versions":
                return await Loaders.ListVersions(S(p, "loader"), S(p, "minecraft"));

            // ---------- Instanzen ----------
            case "instances.list":
                return Instances.List().Select(i => Instances.ToJs(i)).ToList();
            case "instances.get":
                return Instances.ToJs(Instances.Get(S(p, "id")));
            case "instances.create":
                return Instances.ToJs(Instances.Create(S(p, "name"), S(p, "minecraft"), S(p, "loader"), SO(p, "loaderVersion")));
            case "instances.update":
                return Instances.ToJs(Instances.Update(S(p, "id"), SO(p, "name"), (int?)p["memoryMb"],
                    SO(p, "minecraft"), SO(p, "loader"), SO(p, "loaderVersion")));
            case "instances.delete":
                Instances.Delete(S(p, "id"));
                return true;
            case "instances.openFolder":
            {
                var dir = Instances.GameDir(S(p, "id"));
                Directory.CreateDirectory(dir);
                Process.Start(new ProcessStartInfo("explorer.exe", $"\"{dir}\"") { UseShellExecute = true });
                return true;
            }
            case "instances.localMods":
                return Instances.LocalMods(S(p, "id"));
            case "instances.addMods":
            {
                var id = S(p, "id");
                var mods = p["mods"].Deserialize<List<ModEntry>>(Json.Options) ?? new();
                var job = "mods-" + id;
                var ct = StartJob(job);
                var progress = ProgressFor(job);
                try
                {
                    Instance? inst = null;
                    for (int n = 0; n < mods.Count; n++)
                    {
                        progress.Report(new Core.Progress("Mods installieren", n, mods.Count, mods[n].Name));
                        inst = await Instances.AddMod(id, mods[n], ct);
                    }
                    return Instances.ToJs(inst ?? Instances.Get(id));
                }
                finally { EndJob(job); }
            }
            case "instances.removeMod":
                return Instances.ToJs(Instances.RemoveMod(S(p, "id"), S(p, "fileName")));

            // ---------- Pack-Sync ----------
            case "pack.buildManifest":
            {
                // Manifest zum Hochladen: Mods/Loader der Instanz + optional Config-Dateien
                var i = Instances.Get(S(p, "id"));
                var m = i.Manifest;
                return new PackManifest
                {
                    Name = i.Name, Minecraft = m.Minecraft, Loader = m.Loader, LoaderVersion = m.LoaderVersion, Mods = m.Mods,
                    Overrides = (bool?)p["includeConfigs"] == true ? Instances.CollectOverrides(i.Id) : new(),
                };
            }
            case "pack.link":
                return Instances.ToJs(Instances.LinkPack(S(p, "id"), S(p, "packId"), (int)p["version"]!, S(p, "role")));
            case "pack.unlink":
                return Instances.ToJs(Instances.UnlinkPack(S(p, "id")));
            case "pack.apply":
            {
                // Pack-Version anwenden. Ohne "id" wird eine neue Instanz angelegt (Beitreten).
                var manifest = p["manifest"].Deserialize<PackManifest>(Json.Options) ?? throw new ArgumentException("Manifest fehlt");
                var version = (int)p["version"]!;
                var packId = S(p, "packId");
                var role = SO(p, "role") ?? "member";
                var id = SO(p, "id");
                if (id == null)
                {
                    var existing = Instances.List().FirstOrDefault(x => x.PackId == packId);
                    var created = existing ?? Instances.Create(SO(p, "name") ?? manifest.Name, manifest.Minecraft, manifest.Loader, manifest.LoaderVersion);
                    id = created.Id;
                    if (existing == null) Instances.LinkPack(id, packId, 0, role);
                }
                var job = "sync-" + id;
                var ct = StartJob(job);
                try
                {
                    var inst = await Instances.ApplyManifest(id, manifest, version, role, ProgressFor(job), ct);
                    if (inst.PackId == null) inst = Instances.LinkPack(id, packId, version, role);
                    return Instances.ToJs(inst);
                }
                finally { EndJob(job); }
            }

            // ---------- Spielen ----------
            case "game.launch":
            {
                var id = S(p, "id");
                var job = "launch-" + id;
                var ct = StartJob(job);
                try
                {
                    if (AppSettings.Current.LaunchMode != "direct")
                    {
                        var r = await OfficialLauncher.Launch(id, ProgressFor(job), line => _form.Emit("game-log", new { id, line }), ct);
                        if (AppSettings.Current.CloseOnLaunch)
                            _form.BeginInvoke(() => _form.WindowState = FormWindowState.Minimized);
                        return new { mode = "official", r.ProfileName, r.AlreadyRunning };
                    }
                    await GameLauncher.Launch(id, ProgressFor(job),
                        line => _form.Emit("game-log", new { id, line }),
                        code => _form.Emit("game-exit", new { id, code }), ct);
                    _form.Emit("game-started", new { id });
                    if (AppSettings.Current.CloseOnLaunch)
                        _form.BeginInvoke(() => _form.WindowState = FormWindowState.Minimized);
                    return new { mode = "direct" };
                }
                finally { EndJob(job); }
            }
            case "game.kill":
                GameLauncher.Kill(S(p, "id"));
                return true;
            case "game.running":
                return Instances.List().Where(i => GameLauncher.IsRunning(i.Id)).Select(i => i.Id).ToList();

            case "job.cancel":
                lock (_jobs) if (_jobs.TryGetValue(S(p, "job"), out var c)) c.Cancel();
                return true;

            case "shell.open":
                OpenExternal(S(p, "url"));
                return true;

            default:
                throw new ArgumentException($"Unbekannte Methode: {method}");
        }
    }

    public static void OpenExternal(string url)
    {
        if (Uri.TryCreate(url, UriKind.Absolute, out var u) && (u.Scheme == "https" || u.Scheme == "http"))
            Process.Start(new ProcessStartInfo(u.ToString()) { UseShellExecute = true });
    }

    /// Schickt höchstens ~10 Fortschritts-Updates pro Sekunde an die Oberfläche
    class ThrottledProgress : IProgress<Core.Progress>
    {
        readonly Action<Core.Progress> _send;
        long _last;
        public ThrottledProgress(Action<Core.Progress> send) => _send = send;
        public void Report(Core.Progress value)
        {
            var now = Environment.TickCount64;
            if (value.Done != value.Total && value.Done != 0 && now - Interlocked.Read(ref _last) < 100) return;
            Interlocked.Exchange(ref _last, now);
            _send(value);
        }
    }
}
