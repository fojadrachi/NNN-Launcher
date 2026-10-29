namespace PackLink.Core;

public class AppSettings
{
    public const string DefaultServer = "https://nnn-launcher.christianolah56-42e.workers.dev";
    const string OldLocalDefault = "http://localhost:8787";

    public string ServerUrl { get; set; } = DefaultServer;
    // "official" = über den Minecraft Launcher von Mojang (wie CurseForge), "direct" = der Launcher startet selbst
    public string LaunchMode { get; set; } = "official";
    public string OfficialLauncherPath { get; set; } = "";
    public string MsClientId { get; set; } = "";
    public int MemoryMb { get; set; } = 4096;
    public string JvmArgs { get; set; } = "";
    public bool ShowSnapshots { get; set; }
    public bool CloseOnLaunch { get; set; }

    static readonly string FilePath = Path.Combine(AppPaths.Root, "settings.json");
    static AppSettings? _current;

    public static AppSettings Current => _current ??= Load();

    static AppSettings Load()
    {
        var s = Json.Read<AppSettings>(FilePath) ?? new AppSettings();
        // Frühere Standardadresse (lokaler Testserver) auf den Online-Server umstellen
        if (s.ServerUrl == OldLocalDefault) s.ServerUrl = DefaultServer;
        return s;
    }

    public static void Save(AppSettings s)
    {
        s.MemoryMb = Math.Clamp(s.MemoryMb, 1024, 65536);
        s.ServerUrl = s.ServerUrl.Trim().TrimEnd('/');
        _current = s;
        Json.Write(FilePath, s);
    }
}
