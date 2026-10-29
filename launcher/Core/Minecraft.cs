using System.IO.Compression;
using System.Runtime.InteropServices;
using System.Text.RegularExpressions;
using System.Text.Json.Nodes;

namespace PackLink.Core;

/// Zusammengeführte Version (Loader-Profil + Vanilla über "inheritsFrom")
public class ResolvedVersion
{
    public required string Id { get; init; }
    public required string VanillaId { get; init; }
    public required string MainClass { get; init; }
    public required string Type { get; init; }
    public List<JsonObject> Libraries { get; } = new();
    public List<JsonNode> GameArgs { get; } = new();
    public List<JsonNode> JvmArgs { get; } = new();
    public string? LegacyArgs { get; set; }
    public JsonObject? AssetIndex { get; set; }
    public JsonObject? ClientDownload { get; set; }
    public JsonObject? Logging { get; set; }
    public string JavaComponent { get; set; } = "jre-legacy";
    public int JavaMajor { get; set; } = 8;
}

public record LibFile(string Path, string? Url, string? Sha1, long Size, bool IsNative);

static class Minecraft
{
    const string VersionManifest = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
    const string JavaRuntimes = "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";
    const string ResourcesUrl = "https://resources.download.minecraft.net/";

    static JsonNode? _manifest;

    public static async Task<JsonNode> GetManifest(CancellationToken ct = default)
    {
        if (_manifest != null) return _manifest;
        var cacheFile = Path.Combine(AppPaths.Cache, "version_manifest_v2.json");
        try
        {
            var text = await Net.GetString(VersionManifest, ct);
            Directory.CreateDirectory(AppPaths.Cache);
            await File.WriteAllTextAsync(cacheFile, text, ct);
            _manifest = JsonNode.Parse(text)!;
        }
        catch when (File.Exists(cacheFile))
        {
            _manifest = JsonNode.Parse(await File.ReadAllTextAsync(cacheFile, ct))!; // offline
        }
        return _manifest;
    }

    public static async Task<List<object>> ListVersions(bool snapshots, CancellationToken ct = default)
    {
        var m = await GetManifest(ct);
        return m["versions"]!.AsArray()
            .Where(v => snapshots || (string?)v!["type"] == "release")
            .Select(v => (object)new { id = (string)v!["id"]!, type = (string)v["type"]!, releaseTime = (string?)v["releaseTime"] })
            .ToList();
    }

    static string VersionJsonPath(string id) => Path.Combine(AppPaths.Versions, id, id + ".json");

    static async Task<JsonObject> LoadVersionJson(string id, CancellationToken ct)
    {
        var path = VersionJsonPath(id);
        if (File.Exists(path)) return JsonNode.Parse(await File.ReadAllTextAsync(path, ct))!.AsObject();

        var m = await GetManifest(ct);
        var entry = m["versions"]!.AsArray().FirstOrDefault(v => (string?)v!["id"] == id)
            ?? throw new InvalidOperationException($"Minecraft-Version {id} nicht gefunden");
        var url = (string)entry!["url"]!;
        var sha1 = (string?)entry["sha1"];
        await Downloader.DownloadOne(new DlItem(url, path, sha1), ct);
        return JsonNode.Parse(await File.ReadAllTextAsync(path, ct))!.AsObject();
    }

    /// Lädt die Versionskette und führt sie zusammen (Kind überschreibt Eltern).
    public static async Task<ResolvedVersion> Resolve(string id, CancellationToken ct)
    {
        var chain = new List<JsonObject>();
        string? cur = id;
        while (cur != null)
        {
            if (chain.Count > 5) throw new InvalidOperationException("Zu tiefe inheritsFrom-Kette");
            var j = await LoadVersionJson(cur, ct);
            chain.Add(j);
            cur = (string?)j["inheritsFrom"];
        }
        var root = chain[^1];
        var child = chain[0];

        string? Pick(string key) => chain.Select(j => (string?)j[key]).FirstOrDefault(v => v != null);
        JsonObject? PickObj(string key) => chain.Select(j => j[key] as JsonObject).FirstOrDefault(v => v != null);

        var r = new ResolvedVersion
        {
            Id = (string)child["id"]!,
            VanillaId = (string)root["id"]!,
            MainClass = Pick("mainClass") ?? throw new InvalidOperationException("mainClass fehlt"),
            Type = Pick("type") ?? "release",
            AssetIndex = PickObj("assetIndex"),
            ClientDownload = root["downloads"]?["client"] as JsonObject,
            Logging = PickObj("logging")?["client"] as JsonObject,
            LegacyArgs = Pick("minecraftArguments"),
        };
        var java = PickObj("javaVersion");
        if (java != null)
        {
            r.JavaComponent = (string?)java["component"] ?? "jre-legacy";
            r.JavaMajor = (int?)java["majorVersion"] ?? 8;
        }

        // Bibliotheken: Kind zuerst, Duplikate (gleiche Gruppe/Artefakt/Classifier) überspringen
        var seen = new HashSet<string>();
        foreach (var j in chain)
            foreach (var lib in j["libraries"]?.AsArray() ?? new JsonArray())
            {
                var o = lib!.AsObject();
                var key = LibKey((string)o["name"]!);
                if (seen.Add(key)) r.Libraries.Add((JsonObject)o.DeepClone());
            }

        // Argumente: Eltern zuerst, dann Kind
        foreach (var j in Enumerable.Reverse(chain))
        {
            foreach (var a in j["arguments"]?["game"]?.AsArray() ?? new JsonArray()) r.GameArgs.Add(a!.DeepClone());
            foreach (var a in j["arguments"]?["jvm"]?.AsArray() ?? new JsonArray()) r.JvmArgs.Add(a!.DeepClone());
        }
        return r;
    }

    // "group:artifact:version[:classifier][@ext]" -> "group:artifact[:classifier]"
    static string LibKey(string name)
    {
        var parts = name.Split('@')[0].Split(':');
        return parts.Length >= 4 ? $"{parts[0]}:{parts[1]}:{parts[3]}" : $"{parts[0]}:{parts[1]}";
    }

    public static string MavenPath(string name)
    {
        var ext = "jar";
        var at = name.IndexOf('@');
        if (at >= 0) { ext = name[(at + 1)..]; name = name[..at]; }
        var p = name.Split(':');
        var file = $"{p[1]}-{p[2]}" + (p.Length >= 4 ? $"-{p[3]}" : "") + "." + ext;
        return string.Join('/', p[0].Replace('.', '/'), p[1], p[2], file);
    }

    // ---------- Regeln ----------

    static string OsArch => RuntimeInformation.OSArchitecture switch
    {
        Architecture.X64 => "x86_64",
        Architecture.Arm64 => "arm64",
        _ => "x86",
    };

    public static bool RulesAllow(JsonArray? rules, IReadOnlyDictionary<string, bool>? features = null)
    {
        if (rules == null || rules.Count == 0) return true;
        bool allowed = false;
        foreach (var node in rules)
        {
            var rule = node!.AsObject();
            bool match = true;
            if (rule["os"] is JsonObject os)
            {
                if (os["name"] is JsonNode n && (string?)n != "windows") match = false;
                if (os["arch"] is JsonNode a && (string?)a != OsArch && !((string?)a == "x86" && OsArch == "x86")) match = false;
                if (os["version"] is JsonNode v && !Regex.IsMatch(Environment.OSVersion.Version.ToString(), (string)v!)) match = false;
            }
            if (rule["features"] is JsonObject f)
                foreach (var kv in f)
                    if ((features?.GetValueOrDefault(kv.Key) ?? false) != ((bool?)kv.Value ?? false)) match = false;
            if (match) allowed = (string?)rule["action"] == "allow";
        }
        return allowed;
    }

    // ---------- Bibliotheken ----------

    public static List<LibFile> LibraryFiles(ResolvedVersion v)
    {
        var list = new List<LibFile>();
        foreach (var lib in v.Libraries)
        {
            if (!RulesAllow(lib["rules"] as JsonArray)) continue;
            var name = (string)lib["name"]!;
            var downloads = lib["downloads"] as JsonObject;

            // Klassische Natives (bis 1.18): eigene Classifier-Jars, die entpackt werden
            if (lib["natives"]?["windows"] is JsonNode nat)
            {
                var cls = ((string)nat!).Replace("${arch}", Environment.Is64BitOperatingSystem ? "64" : "32");
                if (downloads?["classifiers"]?[cls] is JsonObject c)
                    list.Add(new LibFile(Path.Combine(AppPaths.Libraries, (string)c["path"]!), (string?)c["url"], (string?)c["sha1"], (long?)c["size"] ?? 0, true));
                else
                {
                    var path = MavenPath(name + ":" + cls);
                    list.Add(new LibFile(Path.Combine(AppPaths.Libraries, path), BaseUrl(lib) + path, null, 0, true));
                }
                if (downloads?["artifact"] == null) continue;
            }

            if (downloads?["artifact"] is JsonObject art)
            {
                var path = (string?)art["path"] ?? MavenPath(name);
                var url = (string?)art["url"];
                list.Add(new LibFile(Path.Combine(AppPaths.Libraries, path), string.IsNullOrEmpty(url) ? null : url,
                    (string?)art["sha1"], (long?)art["size"] ?? 0, false));
            }
            else if (downloads == null)
            {
                // Fabric/Quilt/alte Forge-Einträge: nur Maven-Koordinaten + Repository-URL
                var path = MavenPath(name);
                list.Add(new LibFile(Path.Combine(AppPaths.Libraries, path), BaseUrl(lib) + path, (string?)lib["sha1"], (long?)lib["size"] ?? 0, false));
            }
        }
        return list;
    }

    static string BaseUrl(JsonObject lib)
    {
        var u = (string?)lib["url"] ?? "https://libraries.minecraft.net/";
        return u.EndsWith('/') ? u : u + "/";
    }

    public static string VersionJar(ResolvedVersion v) => Path.Combine(AppPaths.Versions, v.Id, v.Id + ".jar");

    /// Stellt sicher, dass alles für den Start vorhanden ist: Client-Jar, Bibliotheken, Assets, Java.
    /// Gibt den Pfad zu java.exe zurück.
    public static async Task<string> EnsureInstalled(ResolvedVersion v, IProgress<Progress>? progress, CancellationToken ct)
    {
        progress?.Report(new Progress("Minecraft-Dateien prüfen", 0, 1));

        // Client-Jar der Vanilla-Version; für Loader-Profile unter deren ID kopiert (wie der offizielle Launcher)
        var vanillaJar = Path.Combine(AppPaths.Versions, v.VanillaId, v.VanillaId + ".jar");
        if (v.ClientDownload != null)
            await Downloader.Run(new[] { new DlItem((string)v.ClientDownload["url"]!, vanillaJar, (string?)v.ClientDownload["sha1"], (long?)v.ClientDownload["size"] ?? 0) },
                "Minecraft herunterladen", progress, ct);
        var jar = VersionJar(v);
        if (jar != vanillaJar && (!File.Exists(jar) || new FileInfo(jar).Length != new FileInfo(vanillaJar).Length))
        {
            Directory.CreateDirectory(Path.GetDirectoryName(jar)!);
            File.Copy(vanillaJar, jar, true);
        }

        var libs = LibraryFiles(v);
        var missingNoUrl = libs.Where(l => l.Url == null && !File.Exists(l.Path)).ToList();
        if (missingNoUrl.Count > 0)
            throw new FileNotFoundException("Bibliothek fehlt (Loader neu installieren): " + Path.GetFileName(missingNoUrl[0].Path));
        await Downloader.Run(libs.Where(l => l.Url != null).Select(l => new DlItem(l.Url!, l.Path, l.Sha1, l.Size)).ToList(),
            "Bibliotheken", progress, ct);

        await EnsureAssets(v, progress, ct);

        if (v.Logging?["file"] is JsonObject log)
            await Downloader.Run(new[] { new DlItem((string)log["url"]!, Path.Combine(AppPaths.Assets, "log_configs", (string)log["id"]!), (string?)log["sha1"], (long?)log["size"] ?? 0) },
                "Log-Konfiguration", progress, ct);

        return await EnsureJava(v.JavaComponent, progress, ct);
    }

    public static string AssetsIndexName(ResolvedVersion v) => (string?)v.AssetIndex?["id"] ?? "legacy";

    /// Gibt für sehr alte Versionen (virtuelle Assets) das virtuelle Verzeichnis zurück, sonst den Assets-Ordner.
    static async Task EnsureAssets(ResolvedVersion v, IProgress<Progress>? progress, CancellationToken ct)
    {
        if (v.AssetIndex == null) return;
        var id = AssetsIndexName(v);
        var indexPath = Path.Combine(AppPaths.Assets, "indexes", id + ".json");
        await Downloader.Run(new[] { new DlItem((string)v.AssetIndex["url"]!, indexPath, (string?)v.AssetIndex["sha1"], (long?)v.AssetIndex["size"] ?? 0) },
            "Asset-Index", progress, ct);
        var index = JsonNode.Parse(await File.ReadAllTextAsync(indexPath, ct))!;
        var objects = index["objects"]!.AsObject();
        var items = new List<DlItem>(objects.Count);
        foreach (var kv in objects)
        {
            var hash = (string)kv.Value!["hash"]!;
            var size = (long)kv.Value["size"]!;
            var sub = hash[..2] + "/" + hash;
            items.Add(new DlItem(ResourcesUrl + sub, Path.Combine(AppPaths.Assets, "objects", hash[..2], hash), hash, size));
        }
        await Downloader.Run(items, "Assets", progress, ct, 24);

        if ((bool?)index["virtual"] == true || (bool?)index["map_to_resources"] == true)
        {
            var virt = Path.Combine(AppPaths.Assets, "virtual", id);
            foreach (var kv in objects)
            {
                var hash = (string)kv.Value!["hash"]!;
                var target = AppPaths.SafeCombine(virt, kv.Key);
                if (File.Exists(target)) continue;
                Directory.CreateDirectory(Path.GetDirectoryName(target)!);
                File.Copy(Path.Combine(AppPaths.Assets, "objects", hash[..2], hash), target);
            }
        }
    }

    public static string GameAssetsDir(ResolvedVersion v)
    {
        var id = AssetsIndexName(v);
        var virt = Path.Combine(AppPaths.Assets, "virtual", id);
        return Directory.Exists(virt) ? virt : AppPaths.Assets;
    }

    // ---------- Java ----------

    public static async Task<string> EnsureJava(string component, IProgress<Progress>? progress, CancellationToken ct)
    {
        var dir = Path.Combine(AppPaths.Runtimes, component);
        var java = Path.Combine(dir, "bin", "java.exe");
        var marker = Path.Combine(dir, ".packlink-ok");
        if (File.Exists(java) && File.Exists(marker)) return java;

        progress?.Report(new Progress($"Java ({component}) herunterladen", 0, 1));
        var all = await Net.GetJson(JavaRuntimes, ct);
        var platform = RuntimeInformation.OSArchitecture == Architecture.Arm64 ? "windows-arm64" : "windows-x64";
        var entries = all[platform]?[component]?.AsArray();
        if (entries == null || entries.Count == 0)
            throw new InvalidOperationException($"Keine Java-Laufzeit '{component}' für {platform} verfügbar");
        var manifest = await Net.GetJson((string)entries[0]!["manifest"]!["url"]!, ct);

        var items = new List<DlItem>();
        foreach (var kv in manifest["files"]!.AsObject())
        {
            var target = AppPaths.SafeCombine(dir, kv.Key);
            var type = (string?)kv.Value!["type"];
            if (type == "directory") { Directory.CreateDirectory(target); continue; }
            if (type != "file") continue;
            var raw = kv.Value["downloads"]!["raw"]!;
            items.Add(new DlItem((string)raw["url"]!, target, (string?)raw["sha1"], (long?)raw["size"] ?? 0));
        }
        await Downloader.Run(items, $"Java ({component})", progress, ct);
        if (!File.Exists(java)) throw new FileNotFoundException("java.exe nach dem Download nicht gefunden");
        await File.WriteAllTextAsync(marker, DateTime.UtcNow.ToString("o"), ct);
        return java;
    }

    // ---------- Natives entpacken ----------

    public static void ExtractNatives(ResolvedVersion v, string targetDir)
    {
        if (Directory.Exists(targetDir)) try { Directory.Delete(targetDir, true); } catch { }
        Directory.CreateDirectory(targetDir);
        foreach (var lib in LibraryFiles(v).Where(l => l.IsNative && File.Exists(l.Path)))
        {
            using var zip = ZipFile.OpenRead(lib.Path);
            foreach (var e in zip.Entries)
            {
                if (e.FullName.StartsWith("META-INF", StringComparison.OrdinalIgnoreCase) || e.FullName.EndsWith('/')) continue;
                var dest = AppPaths.SafeCombine(targetDir, e.FullName);
                Directory.CreateDirectory(Path.GetDirectoryName(dest)!);
                e.ExtractToFile(dest, true);
            }
        }
    }
}
