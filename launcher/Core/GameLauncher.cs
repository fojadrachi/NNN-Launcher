using System.Diagnostics;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;

namespace PackLink.Core;

static class GameLauncher
{
    static readonly Dictionary<string, Process> Running = new();

    public static bool IsRunning(string instanceId)
    {
        lock (Running) return Running.TryGetValue(instanceId, out var p) && !p.HasExited;
    }

    public static void Kill(string instanceId)
    {
        lock (Running)
            if (Running.TryGetValue(instanceId, out var p) && !p.HasExited) p.Kill(true);
    }

    public static async Task Launch(string instanceId, IProgress<Progress> progress, Action<string> log,
        Action<int> onExit, CancellationToken ct)
    {
        if (IsRunning(instanceId)) throw new InvalidOperationException("Diese Instanz läuft bereits");
        var settings = AppSettings.Current;
        var inst = Instances.Get(instanceId);
        var acc = Accounts.Selected ?? throw new InvalidOperationException("Kein Minecraft-Konto angemeldet");
        acc = await MicrosoftAuth.EnsureFresh(acc, settings.MsClientId, ct);

        var m = inst.Manifest;
        var versionId = await Loaders.Install(m.Loader, m.Minecraft, m.LoaderVersion, progress, log, ct);
        var v = await Minecraft.Resolve(versionId, ct);
        var java = await Minecraft.EnsureInstalled(v, progress, ct);

        progress.Report(new Progress("Starten", 1, 1));
        var game = Instances.GameDir(instanceId);
        Directory.CreateDirectory(game);
        var natives = Path.Combine(AppPaths.Natives, instanceId);
        Minecraft.ExtractNatives(v, natives);

        var cp = Minecraft.LibraryFiles(v).Where(l => !l.IsNative).Select(l => l.Path).Distinct(StringComparer.OrdinalIgnoreCase).ToList();
        cp.Add(Minecraft.VersionJar(v));

        var vars = new Dictionary<string, string>
        {
            ["auth_player_name"] = acc.Name,
            ["version_name"] = v.Id,
            ["game_directory"] = game,
            ["assets_root"] = AppPaths.Assets,
            ["game_assets"] = Minecraft.GameAssetsDir(v),
            ["assets_index_name"] = Minecraft.AssetsIndexName(v),
            ["auth_uuid"] = acc.Id,
            ["auth_access_token"] = acc.AccessToken ?? "0",
            ["auth_session"] = acc.AccessToken ?? "0",
            ["auth_xuid"] = acc.Xuid ?? "0",
            ["clientid"] = string.IsNullOrEmpty(settings.MsClientId) ? "0" : settings.MsClientId,
            ["user_type"] = acc.Type == "msa" ? "msa" : "legacy",
            ["user_properties"] = "{}",
            ["version_type"] = v.Type,
            ["natives_directory"] = natives,
            ["launcher_name"] = "NNN Launcher",
            ["launcher_version"] = AppUpdater.CurrentText,
            ["classpath"] = string.Join(';', cp),
            ["classpath_separator"] = ";",
            ["library_directory"] = AppPaths.Libraries,
        };

        var args = new List<string>
        {
            $"-Xmx{inst.MemoryMb ?? settings.MemoryMb}M",
            "-Xms512M",
            "-Dfile.encoding=UTF-8",
        };
        if (!string.IsNullOrWhiteSpace(settings.JvmArgs))
            args.AddRange(SplitArgs(settings.JvmArgs));

        if (v.JvmArgs.Count > 0) args.AddRange(Expand(v.JvmArgs, vars));
        else args.AddRange(new[] { $"-Djava.library.path={natives}", "-cp", vars["classpath"] });

        if (v.Logging?["file"] is JsonObject logFile && v.Logging["argument"] is JsonNode logArg)
            args.Add(((string)logArg!).Replace("${path}", Path.Combine(AppPaths.Assets, "log_configs", (string)logFile["id"]!)));

        args.Add(v.MainClass);
        if (v.GameArgs.Count > 0) args.AddRange(Expand(v.GameArgs, vars));
        else if (v.LegacyArgs != null) args.AddRange(SplitArgs(v.LegacyArgs).Select(a => Subst(a, vars)));

        var psi = new ProcessStartInfo(java)
        {
            WorkingDirectory = game,
            UseShellExecute = false,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            CreateNoWindow = true,
            StandardOutputEncoding = System.Text.Encoding.UTF8,
            StandardErrorEncoding = System.Text.Encoding.UTF8,
        };
        psi.ArgumentList.Add("-Dstdout.encoding=UTF-8");
        psi.ArgumentList.Add("-Dstderr.encoding=UTF-8");
        foreach (var a in args) psi.ArgumentList.Add(a);

        var logPath = Path.Combine(AppPaths.Logs, $"{instanceId}-latest.log");
        var logWriter = new StreamWriter(logPath, false) { AutoFlush = true };
        logWriter.WriteLine($"[NNN] Starte {v.Id} mit {java}");
        var cmdLine = string.Join(' ', args);
        if (acc.Type == "msa" && !string.IsNullOrEmpty(acc.AccessToken)) cmdLine = cmdLine.Replace(acc.AccessToken, "<token>");
        logWriter.WriteLine("[NNN] " + cmdLine);

        var p = new Process { StartInfo = psi, EnableRaisingEvents = true };
        var xml = new Log4jXmlParser();
        void OnLine(string? raw)
        {
            if (raw == null) return;
            lock (xml)
                foreach (var line in xml.Feed(raw))
                {
                    lock (logWriter) logWriter.WriteLine(line);
                    log(line);
                }
        }
        p.OutputDataReceived += (_, e) => OnLine(e.Data);
        p.ErrorDataReceived += (_, e) => OnLine(e.Data);
        p.Exited += (_, _) =>
        {
            lock (Running) Running.Remove(instanceId);
            lock (logWriter) logWriter.Dispose();
            onExit(p.ExitCode);
        };
        p.Start();
        p.BeginOutputReadLine();
        p.BeginErrorReadLine();
        lock (Running) Running[instanceId] = p;

        inst.LastPlayed = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        Instances.Save(inst);
    }

    static readonly IReadOnlyDictionary<string, bool> NoFeatures = new Dictionary<string, bool>();

    static IEnumerable<string> Expand(List<JsonNode> list, Dictionary<string, string> vars)
    {
        foreach (var node in list)
        {
            if (node is JsonValue val) { yield return Subst((string)val!, vars); continue; }
            var o = node.AsObject();
            if (!Minecraft.RulesAllow(o["rules"] as JsonArray, NoFeatures)) continue;
            var value = o["value"];
            if (value is JsonArray arr) foreach (var s in arr) yield return Subst((string)s!, vars);
            else if (value != null) yield return Subst((string)value!, vars);
        }
    }

    static string Subst(string s, Dictionary<string, string> vars) =>
        Regex.Replace(s, @"\$\{(\w+)\}", m => vars.TryGetValue(m.Groups[1].Value, out var r) ? r : m.Value);

    /// Minecraft schreibt mit der Launcher-Log-Konfiguration XML-Events.
    /// Daraus werden normale Zeilen wie "[12:00:01] [Render thread/INFO]: Nachricht".
    class Log4jXmlParser
    {
        readonly System.Text.StringBuilder _buf = new();
        bool _inEvent;

        public IEnumerable<string> Feed(string line)
        {
            if (!_inEvent && !line.TrimStart().StartsWith("<log4j:Event")) return new[] { line };
            _inEvent = true;
            _buf.AppendLine(line);
            if (!line.Contains("</log4j:Event>")) return Array.Empty<string>();
            _inEvent = false;
            var text = _buf.ToString();
            _buf.Clear();
            try
            {
                var ev = System.Xml.Linq.XElement.Parse(text.Replace("log4j:", ""));
                var time = DateTimeOffset.FromUnixTimeMilliseconds(long.Parse((string?)ev.Attribute("timestamp") ?? "0")).ToLocalTime();
                var head = $"[{time:HH:mm:ss}] [{(string?)ev.Attribute("thread")}/{(string?)ev.Attribute("level")}]: ";
                var result = new List<string>();
                var msg = ev.Element("Message")?.Value ?? "";
                result.AddRange(msg.Split('\n').Select((l, i) => i == 0 ? head + l.TrimEnd('\r') : l.TrimEnd('\r')));
                var thr = ev.Element("Throwable")?.Value;
                if (!string.IsNullOrWhiteSpace(thr)) result.AddRange(thr.TrimEnd().Split('\n').Select(l => l.TrimEnd('\r')));
                return result;
            }
            catch { return text.Split('\n').Select(l => l.TrimEnd('\r')).Where(l => l.Length > 0); }
        }
    }

    static IEnumerable<string> SplitArgs(string s) =>
        Regex.Matches(s, "\"([^\"]*)\"|(\\S+)").Select(m => m.Groups[1].Success ? m.Groups[1].Value : m.Groups[2].Value);
}
