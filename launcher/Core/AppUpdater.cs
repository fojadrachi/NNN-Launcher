using System.Diagnostics;
using System.Reflection;
using System.Security.Cryptography;
using System.Text.Json.Nodes;

namespace PackLink.Core;

/// App-Updates über GitHub-Releases: neueste Version prüfen, Setup laden, Prüfsumme checken,
/// still installieren. Das Setup startet den Launcher danach neu.
static class AppUpdater
{
    const string LatestRelease = "https://api.github.com/repos/fojadrachi/NNN-Launcher/releases/latest";
    const string SetupPrefix = "NNN-Launcher-Setup-";

    public static Version Current
    {
        get
        {
            var v = Assembly.GetExecutingAssembly().GetName().Version ?? new Version(1, 0, 0);
            return new Version(v.Major, v.Minor, Math.Max(v.Build, 0));
        }
    }

    public static string CurrentText => Current.ToString(3);

    public record UpdateInfo(bool Available, string Current, string? Latest, string? Notes, string? HtmlUrl,
        string? SetupUrl, long Size, string? Sha256);

    static UpdateInfo? _last;

    public static async Task<UpdateInfo> Check(CancellationToken ct = default)
    {
        using var req = new HttpRequestMessage(HttpMethod.Get, LatestRelease);
        req.Headers.Accept.ParseAdd("application/vnd.github+json");
        using var res = await Net.Client.SendAsync(req, ct);
        if (res.StatusCode == System.Net.HttpStatusCode.NotFound)
            return _last = new UpdateInfo(false, CurrentText, null, null, null, null, 0, null);
        res.EnsureSuccessStatusCode();
        var rel = JsonNode.Parse(await res.Content.ReadAsStringAsync(ct))!;

        var tag = ((string?)rel["tag_name"] ?? "").TrimStart('v', 'V');
        if (!Version.TryParse(tag, out var latest))
            return _last = new UpdateInfo(false, CurrentText, tag, null, null, null, 0, null);

        var asset = rel["assets"]?.AsArray().FirstOrDefault(a =>
            ((string?)a!["name"] ?? "").StartsWith(SetupPrefix, StringComparison.OrdinalIgnoreCase)
            && ((string?)a["name"])!.EndsWith(".exe", StringComparison.OrdinalIgnoreCase));
        var digest = (string?)asset?["digest"]; // "sha256:…", von GitHub automatisch berechnet

        var newer = new Version(latest.Major, latest.Minor, Math.Max(latest.Build, 0)) > Current;
        return _last = new UpdateInfo(newer && asset != null, CurrentText, latest.ToString(3), (string?)rel["body"],
            (string?)rel["html_url"], (string?)asset?["browser_download_url"], (long?)asset?["size"] ?? 0,
            digest != null && digest.StartsWith("sha256:") ? digest[7..].ToLowerInvariant() : null);
    }

    /// Lädt das Setup, prüft es und startet die stille Installation. Danach muss sich die App beenden.
    public static async Task Install(IProgress<Progress> progress, CancellationToken ct)
    {
        var info = _last ?? await Check(ct);
        if (!info.Available || info.SetupUrl == null) throw new InvalidOperationException("Kein Update verfügbar");
        var uri = new Uri(info.SetupUrl);
        if (uri.Scheme != "https" || uri.Host != "github.com")
            throw new InvalidOperationException("Unerwartete Download-Adresse für das Update");

        var file = Path.Combine(AppPaths.Cache, "updates", $"{SetupPrefix}{info.Latest}.exe");
        if (File.Exists(file)) File.Delete(file);
        progress.Report(new Progress("Update herunterladen", 0, 1));
        await Downloader.Run(new[] { new DlItem(info.SetupUrl, file, null, info.Size) }, "Update herunterladen", progress, ct);

        if (info.Sha256 != null)
        {
            await using var s = File.OpenRead(file);
            var hash = Convert.ToHexString(await SHA256.HashDataAsync(s, ct)).ToLowerInvariant();
            if (hash != info.Sha256)
            {
                s.Close();
                File.Delete(file);
                throw new InvalidOperationException("Prüfsumme des Updates stimmt nicht – abgebrochen.");
            }
        }

        progress.Report(new Progress("Update wird installiert …", 1, 1));
        // Still installieren, Meldungen unterdrücken; das Setup startet den Launcher danach selbst neu
        Process.Start(new ProcessStartInfo(file, "/SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS")
        {
            UseShellExecute = true,
        });
    }

    public static bool IsInstalled =>
        Environment.ProcessPath?.StartsWith(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Programs"),
            StringComparison.OrdinalIgnoreCase) == true;
}
