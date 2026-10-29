using System.Reflection;
using System.Text.Json;
using System.Text.Json.Nodes;
using Microsoft.Web.WebView2.Core;
using Microsoft.Web.WebView2.WinForms;
using PackLink.Core;

namespace PackLink;

public class MainForm : Form
{
    const string Host = "app.nnnlauncher";
    readonly WebView2 _web = new() { Dock = DockStyle.Fill, DefaultBackgroundColor = Color.FromArgb(17, 20, 23) };
    readonly Bridge _bridge;
    string? _pendingDeepLink;
    bool _ready;

    public MainForm(string? deepLink)
    {
        _pendingDeepLink = deepLink;
        Text = "NNN Launcher";
        Width = 1280;
        Height = 800;
        MinimumSize = new Size(960, 600);
        StartPosition = FormStartPosition.CenterScreen;
        BackColor = Color.FromArgb(17, 20, 23);
        try { Icon = Icon.ExtractAssociatedIcon(Environment.ProcessPath!); } catch { }
        Controls.Add(_web);
        _bridge = new Bridge(this);
        Load += async (_, _) => await InitWeb();
    }

    async Task InitWeb()
    {
        AppPaths.Ensure();
        try
        {
            var env = await CoreWebView2Environment.CreateAsync(null, AppPaths.WebViewData);
            await _web.EnsureCoreWebView2Async(env);
        }
        catch (WebView2RuntimeNotFoundException)
        {
            MessageBox.Show("Die Microsoft Edge WebView2-Runtime fehlt. Bitte von microsoft.com/edge/webview2 installieren.",
                "NNN Launcher", MessageBoxButtons.OK, MessageBoxIcon.Error);
            Close();
            return;
        }

        var core = _web.CoreWebView2;
        core.Settings.AreDevToolsEnabled = true;
        core.Settings.IsStatusBarEnabled = false;
        core.Settings.AreDefaultContextMenusEnabled = false;
#if DEBUG
        core.Settings.AreDefaultContextMenusEnabled = true;
#endif
        core.AddWebResourceRequestedFilter($"https://{Host}/*", CoreWebView2WebResourceContext.All);
        core.WebResourceRequested += ServeEmbedded;
        core.WebMessageReceived += (_, e) => _bridge.Handle(e.WebMessageAsJson);

        // Externe Links im Standardbrowser öffnen
        core.NewWindowRequested += (_, e) => { e.Handled = true; Bridge.OpenExternal(e.Uri); };
        core.NavigationStarting += (_, e) =>
        {
            if (!e.Uri.StartsWith($"https://{Host}/")) { e.Cancel = true; Bridge.OpenExternal(e.Uri); }
        };

        core.Navigate($"https://{Host}/index.html");
    }

    void ServeEmbedded(object? sender, CoreWebView2WebResourceRequestedEventArgs e)
    {
        var path = new Uri(e.Request.Uri).AbsolutePath.TrimStart('/');
        if (path == "") path = "index.html";
        var stream = Assembly.GetExecutingAssembly().GetManifestResourceStream("wwwroot/" + path);
        if (stream == null)
        {
            e.Response = _web.CoreWebView2.Environment.CreateWebResourceResponse(null, 404, "Not Found", "");
            return;
        }
        var mime = Path.GetExtension(path).ToLowerInvariant() switch
        {
            ".html" => "text/html; charset=utf-8",
            ".js" => "text/javascript; charset=utf-8",
            ".css" => "text/css; charset=utf-8",
            ".svg" => "image/svg+xml",
            ".png" => "image/png",
            ".ico" => "image/x-icon",
            ".woff2" => "font/woff2",
            _ => "application/octet-stream",
        };
        e.Response = _web.CoreWebView2.Environment.CreateWebResourceResponse(stream, 200, "OK",
            $"Content-Type: {mime}\r\nCache-Control: no-cache");
    }

    /// Wird von der Oberfläche aufgerufen, sobald sie geladen ist
    public void OnUiReady()
    {
        _ready = true;
        if (_pendingDeepLink != null) { var l = _pendingDeepLink; _pendingDeepLink = null; HandleDeepLink(l); }
    }

    public void HandleDeepLink(string link)
    {
        if (!_ready) { _pendingDeepLink = link; return; }
        var m = System.Text.RegularExpressions.Regex.Match(link, @"^(?:nnnlauncher|packlink)://join/([A-Za-z0-9]{6,16})", System.Text.RegularExpressions.RegexOptions.IgnoreCase);
        if (!m.Success) return;
        Emit("deep-link", new { code = m.Groups[1].Value });
        BeginInvoke(() =>
        {
            if (WindowState == FormWindowState.Minimized) WindowState = FormWindowState.Normal;
            Activate();
        });
    }

    public void Emit(string name, object? data) => Post(new { @event = name, data });

    public void Post(object message)
    {
        var json = JsonSerializer.Serialize(message, Json.Options);
        if (IsDisposed) return;
        if (InvokeRequired) { try { BeginInvoke(() => Post(json)); } catch { } return; }
        Post(json);
    }

    void Post(string json)
    {
        if (_web.CoreWebView2 != null) _web.CoreWebView2.PostWebMessageAsJson(json);
    }
}
