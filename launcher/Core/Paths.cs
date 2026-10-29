namespace PackLink.Core;

static class AppPaths
{
    public static readonly string Root = ResolveRoot();

    // Früher hieß die App PackLink: vorhandene Daten (Minecraft, Java, Instanzen) einmalig übernehmen
    static string ResolveRoot()
    {
        var appData = Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData);
        var root = Path.Combine(appData, "NNN Launcher");
        var old = Path.Combine(appData, "PackLink");
        if (!Directory.Exists(root) && Directory.Exists(old))
        {
            // Läuft noch etwas aus dem alten Ordner (z. B. Minecraft), klappt der Umzug erst beim nächsten Start
            try { Directory.Move(old, root); }
            catch { return old; }
        }
        return root;
    }

    // Gemeinsame Minecraft-Dateien (wie .minecraft aufgebaut, damit Forge/NeoForge-Installer funktionieren)
    public static readonly string Game = Path.Combine(Root, "minecraft");
    public static readonly string Versions = Path.Combine(Game, "versions");
    public static readonly string Libraries = Path.Combine(Game, "libraries");
    public static readonly string Assets = Path.Combine(Game, "assets");
    public static readonly string Runtimes = Path.Combine(Root, "runtimes");
    public static readonly string Natives = Path.Combine(Root, "natives");
    public static readonly string Cache = Path.Combine(Root, "cache");
    public static readonly string Instances = Path.Combine(Root, "instances");
    public static readonly string WebViewData = Path.Combine(Root, "webview");
    public static readonly string Logs = Path.Combine(Root, "logs");

    public static void Ensure()
    {
        foreach (var d in new[] { Root, Game, Versions, Libraries, Assets, Runtimes, Natives, Cache, Instances, Logs })
            Directory.CreateDirectory(d);
    }

    /// Verhindert, dass relative Pfade aus Manifesten aus dem Zielordner ausbrechen.
    public static string SafeCombine(string baseDir, string relative)
    {
        var full = Path.GetFullPath(Path.Combine(baseDir, relative.Replace('/', Path.DirectorySeparatorChar)));
        var root = Path.GetFullPath(baseDir).TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
        if (!full.StartsWith(root, StringComparison.OrdinalIgnoreCase))
            throw new InvalidOperationException($"Unsicherer Pfad: {relative}");
        return full;
    }
}
