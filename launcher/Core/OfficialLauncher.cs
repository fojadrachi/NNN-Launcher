using System.Diagnostics;
using System.Security.Cryptography.X509Certificates;
using System.Text.Json.Nodes;

namespace PackLink.Core;

/// Start über den offiziellen Minecraft Launcher (wie bei CurseForge):
/// Der NNN Launcher installiert Loader + Mods, schreibt ein Profil in launcher_profiles.json
/// und öffnet den Mojang-Launcher mit unserem Minecraft-Ordner als Arbeitsverzeichnis.
/// Login und Spielstart übernimmt dann der offizielle Launcher.
static class OfficialLauncher
{
    const string DownloadUrl = "https://launcher.mojang.com/download/Minecraft.exe";
    static readonly string BundledExe = Path.Combine(AppPaths.Root, "launcher", "Minecraft.exe");
    static string ProfilesFile => Path.Combine(AppPaths.Game, "launcher_profiles.json");

    public static string ProfileKey(string instanceId) => "nnn-" + instanceId;

    public record Result(string ProfileName, bool AlreadyRunning);

    public static async Task<Result> Launch(string instanceId, IProgress<Progress> progress, Action<string> log, CancellationToken ct)
    {
        var settings = AppSettings.Current;
        var inst = Instances.Get(instanceId);
        var m = inst.Manifest;

        // Loader-Profil (Fabric/Quilt-JSON bzw. Forge/NeoForge-Installer) in unseren Minecraft-Ordner legen.
        // Bibliotheken, Assets und Java lädt danach der offizielle Launcher selbst.
        var versionId = await Loaders.Install(m.Loader, m.Minecraft, m.LoaderVersion, progress, log, ct);

        progress.Report(new Progress("Minecraft Launcher vorbereiten", 0, 1));
        var exe = await FindOrDownload(ct);
        var name = "NNN: " + inst.Name;
        WriteProfile(instanceId, name, versionId, inst.MemoryMb ?? settings.MemoryMb, settings.JvmArgs);

        bool alreadyRunning = Process.GetProcessesByName("MinecraftLauncher").Length > 0
            || Process.GetProcessesByName("Minecraft").Length > 0;

        var psi = new ProcessStartInfo(exe) { UseShellExecute = false, WorkingDirectory = Path.GetDirectoryName(exe)! };
        psi.ArgumentList.Add("--workDir");
        psi.ArgumentList.Add(AppPaths.Game);
        Process.Start(psi);
        log($"[NNN] Minecraft Launcher gestartet ({exe}) mit Profil „{name}“ ({versionId})");

        inst.LastPlayed = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        Instances.Save(inst);
        return new Result(name, alreadyRunning);
    }

    /// Eigene Pfadangabe aus den Einstellungen, sonst installierter Launcher, sonst Mojangs Download.
    static async Task<string> FindOrDownload(CancellationToken ct)
    {
        var custom = AppSettings.Current.OfficialLauncherPath;
        if (!string.IsNullOrWhiteSpace(custom))
        {
            if (!File.Exists(custom)) throw new FileNotFoundException("Der eingestellte Minecraft Launcher wurde nicht gefunden: " + custom);
            return custom;
        }

        var candidates = new[]
        {
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86), "Minecraft Launcher", "MinecraftLauncher.exe"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "Minecraft Launcher", "MinecraftLauncher.exe"),
            BundledExe,
        };
        foreach (var c in candidates)
            if (File.Exists(c)) return c;

        await Downloader.DownloadOne(new DlItem(DownloadUrl, BundledExe), ct);
        if (!IsSignedByMojang(BundledExe))
        {
            File.Delete(BundledExe);
            throw new InvalidOperationException("Heruntergeladener Minecraft Launcher ist nicht von Mojang/Microsoft signiert – abgebrochen.");
        }
        return BundledExe;
    }

    static bool IsSignedByMojang(string file)
    {
        try
        {
#pragma warning disable SYSLIB0057
            var cert = new X509Certificate2(X509Certificate.CreateFromSignedFile(file));
#pragma warning restore SYSLIB0057
            var subject = cert.Subject;
            return subject.Contains("Mojang", StringComparison.OrdinalIgnoreCase)
                || subject.Contains("Microsoft", StringComparison.OrdinalIgnoreCase);
        }
        catch { return false; }
    }

    static void WriteProfile(string instanceId, string name, string versionId, int memoryMb, string extraArgs)
    {
        JsonObject root;
        try { root = File.Exists(ProfilesFile) ? JsonNode.Parse(File.ReadAllText(ProfilesFile))!.AsObject() : new JsonObject(); }
        catch { root = new JsonObject(); }
        if (root["profiles"] is not JsonObject profiles) root["profiles"] = profiles = new JsonObject();
        root["settings"] ??= new JsonObject();
        root["version"] ??= 3;

        // Der Ordner gehört nur dem NNN Launcher: Profile, die Forge/NeoForge-Installer anlegen, und Profile
        // gelöschter Instanzen entfernen – sonst stehen sie im offiziellen Launcher vor unserem Profil.
        foreach (var key in profiles.Select(kv => kv.Key).ToList())
        {
            var type = (string?)profiles[key]?["type"];
            if (key.StartsWith("nnn-") ? !Directory.Exists(Instances.Dir(key["nnn-".Length..])) : type == "custom")
                profiles.Remove(key);
        }

        var now = DateTime.UtcNow.ToString("yyyy-MM-ddTHH:mm:ss.fffZ");
        var key2 = ProfileKey(instanceId);
        var created = (string?)profiles[key2]?["created"] ?? now;
        var args = $"-Xmx{memoryMb}M -Xms512M -XX:+UseG1GC -XX:+UnlockExperimentalVMOptions -XX:G1NewSizePercent=20 -XX:G1ReservePercent=20 -XX:MaxGCPauseMillis=50 -XX:G1HeapRegionSize=32M";
        if (!string.IsNullOrWhiteSpace(extraArgs)) args += " " + extraArgs.Trim();

        // Neuestes lastUsed = im offiziellen Launcher vorausgewählt
        profiles[key2] = new JsonObject
        {
            ["name"] = name,
            ["type"] = "custom",
            ["created"] = created,
            ["lastUsed"] = now,
            ["lastVersionId"] = versionId,
            ["gameDir"] = Instances.GameDir(instanceId),
            ["javaArgs"] = args,
            ["icon"] = "Crafting_Table",
        };

        var tmp = ProfilesFile + ".tmp";
        Directory.CreateDirectory(AppPaths.Game);
        File.WriteAllText(tmp, root.ToJsonString(new System.Text.Json.JsonSerializerOptions { WriteIndented = true }));
        File.Move(tmp, ProfilesFile, true);
    }
}
