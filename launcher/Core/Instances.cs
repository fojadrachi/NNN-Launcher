using System.Text.Json;

namespace PackLink.Core;

public class ModEntry
{
    public string Source { get; set; } = "modrinth";   // "modrinth" | "curseforge" | "url"
    public string ProjectId { get; set; } = "";
    public string VersionId { get; set; } = "";
    public string Name { get; set; } = "";
    public string? VersionName { get; set; }
    public string FileName { get; set; } = "";
    public string Url { get; set; } = "";
    public string? Sha1 { get; set; }
    public long Size { get; set; }
    public string? IconUrl { get; set; }
    public string? Side { get; set; }
    public bool Dependency { get; set; }
}

public class OverrideEntry
{
    public string Path { get; set; } = "";
    public string Data { get; set; } = "";  // Base64
    public string Sha1 { get; set; } = "";
}

public class PackManifest
{
    public int FormatVersion { get; set; } = 1;
    public string Name { get; set; } = "";
    public string Minecraft { get; set; } = "";
    public string Loader { get; set; } = "vanilla";
    public string? LoaderVersion { get; set; }
    public List<ModEntry> Mods { get; set; } = new();
    public List<OverrideEntry> Overrides { get; set; } = new();
}

public class Instance
{
    public string Id { get; set; } = "";
    public string Name { get; set; } = "";
    public PackManifest Manifest { get; set; } = new();
    // Verknüpfung mit einem geteilten Pack auf dem Server
    public string? PackId { get; set; }
    public int PackVersion { get; set; }
    public string? PackRole { get; set; }
    public string? PublishedHash { get; set; }
    public List<string> ManagedFiles { get; set; } = new();
    public int? MemoryMb { get; set; }
    public long CreatedAt { get; set; }
    public long LastPlayed { get; set; }
}

static class Instances
{
    static readonly SemaphoreSlim Gate = new(1, 1);

    public static string Dir(string id) => System.IO.Path.Combine(AppPaths.Instances, id);
    public static string GameDir(string id) => System.IO.Path.Combine(Dir(id), "game");
    static string FilePath(string id) => System.IO.Path.Combine(Dir(id), "instance.json");

    static void CheckId(string id)
    {
        if (!System.Text.RegularExpressions.Regex.IsMatch(id ?? "", "^[a-z0-9-]{1,40}$")) throw new ArgumentException("Ungültige Instanz-ID");
    }

    public static List<Instance> List()
    {
        var list = new List<Instance>();
        if (!Directory.Exists(AppPaths.Instances)) return list;
        foreach (var d in Directory.GetDirectories(AppPaths.Instances))
        {
            try { var i = Json.Read<Instance>(System.IO.Path.Combine(d, "instance.json")); if (i != null) list.Add(i); }
            catch { }
        }
        return list.OrderByDescending(i => Math.Max(i.LastPlayed, i.CreatedAt)).ToList();
    }

    public static Instance Get(string id)
    {
        CheckId(id);
        return Json.Read<Instance>(FilePath(id)) ?? throw new FileNotFoundException("Instanz nicht gefunden");
    }

    public static void Save(Instance i) => Json.Write(FilePath(i.Id), i);

    public static Instance Create(string name, string mc, string loader, string? loaderVersion)
    {
        name = name.Trim();
        if (name.Length == 0) throw new ArgumentException("Name fehlt");
        if (!Loaders.All.Contains(loader)) throw new ArgumentException("Unbekannter Loader");
        var slug = new string(name.ToLowerInvariant().Select(c => char.IsAsciiLetterOrDigit(c) ? c : '-').ToArray()).Trim('-');
        if (slug.Length == 0) slug = "instanz";
        if (slug.Length > 30) slug = slug[..30];
        var id = slug;
        for (int n = 2; Directory.Exists(Dir(id)); n++) id = $"{slug}-{n}";
        var inst = new Instance
        {
            Id = id,
            Name = name,
            Manifest = new PackManifest { Name = name, Minecraft = mc, Loader = loader, LoaderVersion = loader == "vanilla" ? null : loaderVersion },
            CreatedAt = DateTimeOffset.UtcNow.ToUnixTimeSeconds(),
        };
        Directory.CreateDirectory(System.IO.Path.Combine(GameDir(id), "mods"));
        Save(inst);
        return inst;
    }

    public static void Delete(string id)
    {
        CheckId(id);
        if (Directory.Exists(Dir(id))) Directory.Delete(Dir(id), true);
    }

    public static Instance Update(string id, string? name, int? memoryMb, string? mc, string? loader, string? loaderVersion)
    {
        var i = Get(id);
        if (!string.IsNullOrWhiteSpace(name)) { i.Name = name.Trim(); i.Manifest.Name = i.Name; }        if (memoryMb.HasValue) i.MemoryMb = memoryMb.Value <= 0 ? null : Math.Clamp(memoryMb.Value, 1024, 65536);
        if (!string.IsNullOrEmpty(mc)) i.Manifest.Minecraft = mc;
        if (!string.IsNullOrEmpty(loader))
        {
            if (!Loaders.All.Contains(loader)) throw new ArgumentException("Unbekannter Loader");
            i.Manifest.Loader = loader;
            i.Manifest.LoaderVersion = loader == "vanilla" ? null : loaderVersion;
        }
        Save(i);
        return i;
    }

    // ---------- Mods ----------

    static string ModPath(string id, ModEntry m) => AppPaths.SafeCombine(GameDir(id), "mods/" + m.FileName);

    public static async Task<Instance> AddMod(string id, ModEntry mod, CancellationToken ct)
    {
        if (string.IsNullOrEmpty(mod.Url) || !mod.Url.StartsWith("https://"))
            throw new InvalidOperationException($"{mod.Name}: Kein Download erlaubt (der Autor hat Fremd-Downloads gesperrt).");
        var target = ModPath(id, mod);
        await Downloader.Run(new[] { new DlItem(mod.Url, target, mod.Sha1, mod.Size) }, "Mod herunterladen", null, ct);

        await Gate.WaitAsync(ct);
        try
        {
            var i = Get(id);
            // Alte Version desselben Projekts entfernen
            foreach (var old in i.Manifest.Mods.Where(m => SameProject(m, mod)).ToList())
            {
                if (!old.FileName.Equals(mod.FileName, StringComparison.OrdinalIgnoreCase)) TryDelete(ModPath(id, old));
                i.Manifest.Mods.Remove(old);
                i.ManagedFiles.Remove("mods/" + old.FileName);
            }
            i.Manifest.Mods.Add(mod);
            i.ManagedFiles.Add("mods/" + mod.FileName);
            Save(i);
            return i;
        }
        finally { Gate.Release(); }
    }

    static bool SameProject(ModEntry a, ModEntry b) =>
        (a.Source == b.Source && a.ProjectId == b.ProjectId && a.ProjectId != "")
        || a.FileName.Equals(b.FileName, StringComparison.OrdinalIgnoreCase);

    public static Instance RemoveMod(string id, string fileName)
    {
        var i = Get(id);
        var mod = i.Manifest.Mods.FirstOrDefault(m => m.FileName.Equals(fileName, StringComparison.OrdinalIgnoreCase));
        TryDelete(AppPaths.SafeCombine(GameDir(id), "mods/" + fileName));
        if (mod != null) i.Manifest.Mods.Remove(mod);
        i.ManagedFiles.Remove("mods/" + fileName);
        Save(i);
        return i;
    }

    /// Jar-Dateien im mods-Ordner, die nicht im Manifest stehen (selbst reinkopiert)
    public static List<object> LocalMods(string id)
    {
        var i = Get(id);
        var dir = System.IO.Path.Combine(GameDir(id), "mods");
        if (!Directory.Exists(dir)) return new();
        var known = i.Manifest.Mods.Select(m => m.FileName).ToHashSet(StringComparer.OrdinalIgnoreCase);
        return Directory.GetFiles(dir, "*.jar")
            .Select(System.IO.Path.GetFileName)
            .Where(f => !known.Contains(f!))
            .Select(f => (object)new { fileName = f, size = new FileInfo(System.IO.Path.Combine(dir, f!)).Length })
            .ToList();
    }

    // ---------- Sync ----------

    static readonly string[] OverrideFolders = { "config", "defaultconfigs", "kubejs", "scripts" };
    const long MaxOverrideBytes = 1_100_000;

    /// Sammelt Config-Dateien zum Mitschicken in einem Pack-Update.
    public static List<OverrideEntry> CollectOverrides(string id)
    {
        var game = GameDir(id);
        var list = new List<OverrideEntry>();
        long total = 0;
        foreach (var folder in OverrideFolders)
        {
            var dir = System.IO.Path.Combine(game, folder);
            if (!Directory.Exists(dir)) continue;
            foreach (var file in Directory.EnumerateFiles(dir, "*", SearchOption.AllDirectories).OrderBy(f => f))
            {
                var info = new FileInfo(file);
                if (info.Length > 256_000) continue;
                var ext = info.Extension.ToLowerInvariant();
                if (ext is ".bak" or ".log" or ".tmp" or ".lock") continue;
                total += info.Length;
                if (total > MaxOverrideBytes)
                    throw new InvalidOperationException("Config-Ordner zu groß zum Mitschicken (max. ca. 1 MB).");
                var bytes = File.ReadAllBytes(file);
                list.Add(new OverrideEntry
                {
                    Path = System.IO.Path.GetRelativePath(game, file).Replace('\\', '/'),
                    Data = Convert.ToBase64String(bytes),
                    Sha1 = Hash.Sha1(bytes),
                });
            }
        }
        return list;
    }

    public static string ManifestHash(PackManifest m)
    {
        var copy = new { m.Minecraft, m.Loader, m.LoaderVersion, Mods = m.Mods.Select(x => x.FileName + ":" + x.Sha1).OrderBy(x => x) };
        return Hash.Sha256(JsonSerializer.Serialize(copy));
    }

    /// Wendet ein Pack-Manifest auf eine Instanz an: lädt neue/geänderte Dateien, entfernt nicht mehr
    /// enthaltene (nur die von PackLink verwalteten – eigene Mods des Nutzers bleiben).
    public static async Task<Instance> ApplyManifest(string id, PackManifest manifest, int packVersion, string? role,
        IProgress<Progress>? progress, CancellationToken ct)
    {
        var i = Get(id);
        var game = GameDir(id);
        var wanted = new HashSet<string>(StringComparer.OrdinalIgnoreCase);

        var downloads = new List<DlItem>();
        foreach (var mod in manifest.Mods)
        {
            if (string.IsNullOrEmpty(mod.Url)) throw new InvalidOperationException($"{mod.Name}: keine Download-URL");
            var rel = "mods/" + mod.FileName;
            wanted.Add(rel);
            downloads.Add(new DlItem(mod.Url, AppPaths.SafeCombine(game, rel), mod.Sha1, mod.Size));
        }
        foreach (var o in manifest.Overrides) wanted.Add(o.Path);

        // Erst alles herunterladen, dann löschen – so bleibt die Instanz bei einem Fehler spielbar
        await Downloader.Run(downloads, "Mods synchronisieren", progress, ct, 8);

        foreach (var o in manifest.Overrides)
        {
            var path = AppPaths.SafeCombine(game, o.Path);
            if (File.Exists(path) && Hash.Sha1File(path) == o.Sha1) continue;
            Directory.CreateDirectory(System.IO.Path.GetDirectoryName(path)!);
            File.WriteAllBytes(path, Convert.FromBase64String(o.Data));
        }

        foreach (var old in i.ManagedFiles.Where(f => !wanted.Contains(f)))
            TryDelete(AppPaths.SafeCombine(game, old));

        // Overrides nicht im lokalen Manifest speichern (nur Mods + Loader), spart Platz
        i.Manifest = new PackManifest
        {
            Name = i.Name, Minecraft = manifest.Minecraft, Loader = manifest.Loader,
            LoaderVersion = manifest.LoaderVersion, Mods = manifest.Mods,
        };
        i.ManagedFiles = wanted.ToList();
        i.PackVersion = packVersion;
        if (role != null) i.PackRole = role;
        i.PublishedHash = ManifestHash(i.Manifest);
        Save(i);
        return i;
    }

    public static Instance LinkPack(string id, string packId, int version, string role)
    {
        var i = Get(id);
        i.PackId = packId;
        i.PackVersion = version;
        i.PackRole = role;
        i.PublishedHash = ManifestHash(i.Manifest);
        Save(i);
        return i;
    }

    public static Instance UnlinkPack(string id)
    {
        var i = Get(id);
        i.PackId = null; i.PackVersion = 0; i.PackRole = null; i.PublishedHash = null;
        Save(i);
        return i;
    }

    public static object ToJs(Instance i) => new
    {
        i.Id, i.Name, i.Manifest.Minecraft, i.Manifest.Loader, i.Manifest.LoaderVersion,
        mods = i.Manifest.Mods,
        i.PackId, i.PackVersion, i.PackRole, i.MemoryMb, i.CreatedAt, i.LastPlayed,
        dirty = i.PackId != null && i.PublishedHash != ManifestHash(i.Manifest),
    };

    static void TryDelete(string p) { try { if (File.Exists(p)) File.Delete(p); } catch { } }
}
