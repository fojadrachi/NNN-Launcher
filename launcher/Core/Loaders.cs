using System.Diagnostics;
using System.IO.Compression;
using System.Text.Json.Nodes;
using System.Xml.Linq;

namespace PackLink.Core;

/// Installiert Fabric, Quilt, Forge und NeoForge und liefert die Versions-ID zum Starten.
static class Loaders
{
    public static readonly string[] All = { "vanilla", "fabric", "quilt", "forge", "neoforge" };

    // ---------- Versionslisten ----------

    public static async Task<List<object>> ListVersions(string loader, string mc, CancellationToken ct = default)
    {
        switch (loader)
        {
            case "fabric":
            {
                var arr = (await Net.GetJson($"https://meta.fabricmc.net/v2/versions/loader/{mc}", ct)).AsArray();
                return arr.Select(e => (object)new { version = (string)e!["loader"]!["version"]!, stable = (bool?)e["loader"]!["stable"] ?? false }).ToList();
            }
            case "quilt":
            {
                var arr = (await Net.GetJson($"https://meta.quiltmc.org/v3/versions/loader/{mc}", ct)).AsArray();
                return arr.Select(e =>
                {
                    var v = (string)e!["loader"]!["version"]!;
                    return (object)new { version = v, stable = !v.Contains('-') };
                }).ToList();
            }
            case "forge":
            {
                var xml = XDocument.Parse(await Net.GetString("https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml", ct));
                var promos = await Net.GetJson("https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json", ct);
                var rec = (string?)promos["promos"]?[$"{mc}-recommended"];
                return xml.Descendants("version").Select(x => x.Value)
                    .Where(v => v.StartsWith(mc + "-"))
                    .Select(v => v[(mc.Length + 1)..])
                    .Select(v => (object)new { version = v, stable = v == rec, recommended = v == rec })
                    .ToList();
            }
            case "neoforge":
            {
                if (mc == "1.20.1")
                {
                    var old = await Net.GetJson("https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/forge", ct);
                    return old["versions"]!.AsArray().Select(v => (string)v!)
                        .Where(v => v.StartsWith("1.20.1-")).Reverse()
                        .Select(v => (object)new { version = v[7..], stable = true }).ToList();
                }
                var data = await Net.GetJson("https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge", ct);
                var prefix = NeoForgePrefix(mc);
                return data["versions"]!.AsArray().Select(v => (string)v!)
                    .Where(v => v.StartsWith(prefix)).Reverse()
                    .Select(v => (object)new { version = v, stable = !v.Contains("beta") }).ToList();
            }
            default:
                return new List<object>();
        }
    }

    // 1.21.1 -> "21.1.", 1.21 -> "21.0.", 1.20.4 -> "20.4."
    static string NeoForgePrefix(string mc)
    {
        var p = mc.Split('.');
        return $"{p[1]}.{(p.Length > 2 ? p[2] : "0")}.";
    }

    // ---------- Installation ----------

    public static async Task<string> Install(string loader, string mc, string? loaderVersion,
        IProgress<Progress>? progress, Action<string>? log, CancellationToken ct)
    {
        if (loader == "vanilla" || string.IsNullOrEmpty(loader)) return mc;
        if (string.IsNullOrEmpty(loaderVersion)) throw new InvalidOperationException("Keine Loader-Version gewählt");
        return loader switch
        {
            "fabric" => await InstallProfile($"fabric-loader-{loaderVersion}-{mc}",
                $"https://meta.fabricmc.net/v2/versions/loader/{mc}/{loaderVersion}/profile/json", ct),
            "quilt" => await InstallProfile($"quilt-loader-{loaderVersion}-{mc}",
                $"https://meta.quiltmc.org/v3/versions/loader/{mc}/{loaderVersion}/profile/json", ct),
            "forge" => await InstallForgeLike(
                $"https://maven.minecraftforge.net/net/minecraftforge/forge/{mc}-{loaderVersion}/forge-{mc}-{loaderVersion}-installer.jar",
                mc, progress, log, ct),
            "neoforge" => await InstallForgeLike(mc == "1.20.1"
                    ? $"https://maven.neoforged.net/releases/net/neoforged/forge/1.20.1-{loaderVersion}/forge-1.20.1-{loaderVersion}-installer.jar"
                    : $"https://maven.neoforged.net/releases/net/neoforged/neoforge/{loaderVersion}/neoforge-{loaderVersion}-installer.jar",
                mc, progress, log, ct),
            _ => throw new InvalidOperationException($"Unbekannter Loader: {loader}"),
        };
    }

    static async Task<string> InstallProfile(string id, string url, CancellationToken ct)
    {
        var path = Path.Combine(AppPaths.Versions, id, id + ".json");
        if (File.Exists(path)) return id;
        var json = (await Net.GetJson(url, ct)).AsObject();
        json["id"] = id;
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        await File.WriteAllTextAsync(path, json.ToJsonString(), ct);
        return id;
    }

    static async Task<string> InstallForgeLike(string installerUrl, string mc, IProgress<Progress>? progress,
        Action<string>? log, CancellationToken ct)
    {
        var installer = Path.Combine(AppPaths.Cache, "installers", Path.GetFileName(new Uri(installerUrl).LocalPath));
        progress?.Report(new Progress("Loader-Installer herunterladen", 0, 1));
        await Downloader.Run(new[] { new DlItem(installerUrl, installer) }, "Loader-Installer", progress, ct);

        string id;
        using (var zip = ZipFile.OpenRead(installer))
        {
            var entry = zip.GetEntry("version.json") ?? throw new InvalidDataException("Installer enthält keine version.json");
            using var s = entry.Open();
            id = (string)JsonNode.Parse(s)!["id"]!;
        }

        var versionJson = Path.Combine(AppPaths.Versions, id, id + ".json");
        var marker = Path.Combine(AppPaths.Versions, id, ".packlink-installed");
        if (File.Exists(versionJson) && File.Exists(marker)) return id;

        // Vanilla vorher laden, das spart dem Installer Arbeit und liefert die passende Java-Version
        var vanilla = await Minecraft.Resolve(mc, ct);
        var java = await Minecraft.EnsureInstalled(vanilla, progress, ct);

        var profiles = Path.Combine(AppPaths.Game, "launcher_profiles.json");
        if (!File.Exists(profiles)) await File.WriteAllTextAsync(profiles, "{\"profiles\":{}}", ct);

        progress?.Report(new Progress("Loader installieren (kann ein paar Minuten dauern)", 0, 1));
        // Forge nutzt --installClient, NeoForge --install-client; wir probieren beide
        var (code, output) = await RunJava(java, new[] { "-jar", installer, "--installClient", AppPaths.Game }, log, ct);
        if (code != 0 && output.Contains("installClient", StringComparison.OrdinalIgnoreCase))
            (code, output) = await RunJava(java, new[] { "-jar", installer, "--install-client", AppPaths.Game }, log, ct);
        if (code != 0 || !File.Exists(versionJson))
            throw new InvalidOperationException($"Loader-Installation fehlgeschlagen (Code {code}). Details im Log.");

        await File.WriteAllTextAsync(marker, DateTime.UtcNow.ToString("o"), ct);
        return id;
    }

    static async Task<(int, string)> RunJava(string java, string[] args, Action<string>? log, CancellationToken ct)
    {
        var psi = new ProcessStartInfo(java)
        {
            WorkingDirectory = AppPaths.Cache,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        foreach (var a in args) psi.ArgumentList.Add(a);
        using var p = Process.Start(psi)!;
        var sb = new System.Text.StringBuilder();
        void On(string? l) { if (l == null) return; lock (sb) sb.AppendLine(l); log?.Invoke(l); }
        p.OutputDataReceived += (_, e) => On(e.Data);
        p.ErrorDataReceived += (_, e) => On(e.Data);
        p.BeginOutputReadLine();
        p.BeginErrorReadLine();
        try { await p.WaitForExitAsync(ct); }
        catch (OperationCanceledException) { try { p.Kill(true); } catch { } throw; }
        return (p.ExitCode, sb.ToString());
    }
}
