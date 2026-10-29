using System.IO.Pipes;
using Microsoft.Win32;

namespace PackLink;

static class Program
{
    const string PipeName = "NNNLauncher-DeepLink";

    [STAThread]
    static void Main(string[] args)
    {
        if (args.Length >= 3 && args[0] == "--selftest")
        {
            Environment.Exit(SelfTest.Run(args[1], args[2], args.Length > 3 ? args[3] : null).GetAwaiter().GetResult());
            return;
        }

        var deepLink = args.FirstOrDefault(a => a.StartsWith("nnnlauncher://", StringComparison.OrdinalIgnoreCase) || a.StartsWith("packlink://", StringComparison.OrdinalIgnoreCase));

        using var mutex = new Mutex(true, "NNNLauncher-SingleInstance", out bool first);
        if (!first)
        {
            // Läuft schon: Link an die offene Instanz weitergeben
            if (deepLink != null)
            {
                try
                {
                    using var client = new NamedPipeClientStream(".", PipeName, PipeDirection.Out);
                    client.Connect(2000);
                    using var w = new StreamWriter(client);
                    w.WriteLine(deepLink);
                }
                catch { }
            }
            return;
        }

        RegisterProtocol();
        ApplicationConfiguration.Initialize();
        var form = new MainForm(deepLink);
        StartPipeServer(form);
        Application.Run(form);
    }

    // nnnlauncher://join/CODE für den aktuellen Nutzer registrieren (HKCU, keine Adminrechte nötig).
    // packlink:// zeigt ebenfalls hierher, damit schon verschickte Links weiter funktionieren.
    static void RegisterProtocol()
    {
        var exe = Environment.ProcessPath!;
        foreach (var scheme in new[] { "nnnlauncher", "packlink" })
        {
            try
            {
                using var key = Registry.CurrentUser.CreateSubKey($@"Software\Classes\{scheme}");
                key.SetValue("", "URL:NNN Launcher");
                key.SetValue("URL Protocol", "");
                using var icon = key.CreateSubKey("DefaultIcon");
                icon.SetValue("", $"\"{exe}\",0");
                using var cmd = key.CreateSubKey(@"shell\open\command");
                cmd.SetValue("", $"\"{exe}\" \"%1\"");
            }
            catch { }
        }
    }

    static void StartPipeServer(MainForm form)
    {
        var t = new Thread(() =>
        {
            while (true)
            {
                try
                {
                    using var server = new NamedPipeServerStream(PipeName, PipeDirection.In);
                    server.WaitForConnection();
                    using var r = new StreamReader(server);
                    var line = r.ReadLine();
                    if (!string.IsNullOrWhiteSpace(line)) form.HandleDeepLink(line.Trim());
                }
                catch { Thread.Sleep(500); }
            }
        }) { IsBackground = true };
        t.Start();
    }
}
