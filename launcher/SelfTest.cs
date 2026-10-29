using PackLink.Core;

namespace PackLink;

/// Ohne Oberfläche: Instanz anlegen, Minecraft + Loader installieren, starten und beenden,
/// sobald das Spiel hochgefahren ist. Aufruf: "NNN Launcher.exe" --selftest 1.20.1 fabric 0.16.9
/// Ausgabe in %APPDATA%\PackLink\logs\selftest.log
static class SelfTest
{
    public static async Task<int> Run(string mc, string loader, string? loaderVersion)
    {
        AppPaths.Ensure();
        var logPath = Path.Combine(AppPaths.Logs, "selftest.log");
        using var log = new StreamWriter(logPath, false) { AutoFlush = true };
        void W(string s) { lock (log) log.WriteLine($"{DateTime.Now:HH:mm:ss} {s}"); }

        // Eigenes Konto-Setup ohne bestehende Konten anzufassen
        var previous = Accounts.Store.SelectedId;
        var hadTestAcc = Accounts.Store.Accounts.Any(a => a.Name == "NNNTest");
        try
        {
            W($"Selbsttest {mc} {loader} {loaderVersion}");
            Accounts.AddOffline("NNNTest");
            var id = $"selftest-{loader}-{mc.Replace('.', '-')}";
            if (!Directory.Exists(Instances.Dir(id)))
            {
                var inst = Instances.Create(id, mc, loader, loaderVersion);
                id = inst.Id;
            }

            var started = new TaskCompletionSource<bool>();
            var exited = new TaskCompletionSource<int>();
            string lastStage = "";
            var progress = new Progress<Core.Progress>(p =>
            {
                if (p.Stage != lastStage) { lastStage = p.Stage; W($"[Fortschritt] {p.Stage} ({p.Total})"); }
            });
            await GameLauncher.Launch(id, progress, line =>
            {
                W("[Spiel] " + line);
                if (line.Contains("Sound engine started") || line.Contains("Created: ") && line.Contains("textures-atlas")
                    || line.Contains("Backend library: LWJGL"))
                    started.TrySetResult(true);
            }, code => exited.TrySetResult(code), CancellationToken.None);
            W("Prozess gestartet, warte auf Spiel …");

            var done = await Task.WhenAny(started.Task, exited.Task, Task.Delay(TimeSpan.FromMinutes(4)));
            if (done == started.Task)
            {
                // Noch kurz laufen lassen, damit Mods geladen werden
                await Task.WhenAny(exited.Task, Task.Delay(TimeSpan.FromSeconds(15)));
                if (exited.Task.IsCompleted) { W($"FEHLER: Spiel nach Start beendet (Code {exited.Task.Result})"); return 1; }
                GameLauncher.Kill(id);
                W("OK: Spiel ist gestartet");
                return 0;
            }
            if (done == exited.Task) { W($"FEHLER: Spiel beendet mit Code {exited.Task.Result}"); return 1; }
            GameLauncher.Kill(id);
            W("FEHLER: Zeitüberschreitung");
            return 2;
        }
        catch (Exception e)
        {
            W("FEHLER: " + e);
            return 3;
        }
        finally
        {
            if (!hadTestAcc)
            {
                var t = Accounts.Store.Accounts.FirstOrDefault(a => a.Name == "NNNTest");
                if (t != null) Accounts.Remove(t.Id);
            }
            if (previous != null) Accounts.Select(previous);
        }
    }
}
