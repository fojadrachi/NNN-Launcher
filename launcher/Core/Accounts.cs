using System.Net.Http.Json;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace PackLink.Core;

public class Account
{
    public string Id { get; set; } = "";          // Minecraft-UUID ohne Bindestriche
    public string Name { get; set; } = "";
    public string Type { get; set; } = "offline"; // "msa" | "offline"
    public string? AccessToken { get; set; }
    public long AccessExpires { get; set; }
    public string? RefreshToken { get; set; }
    public string? Xuid { get; set; }
}

public class AccountStore
{
    public List<Account> Accounts { get; set; } = new();
    public string? SelectedId { get; set; }
    // Sitzung beim PackLink-Server
    public string? ServerToken { get; set; }
    public JsonObject? ServerUser { get; set; }
}

/// Konten werden per Windows-DPAPI verschlüsselt gespeichert (nur dieser Windows-Nutzer kann sie lesen).
static class Accounts
{
    static readonly string FilePath = Path.Combine(AppPaths.Root, "accounts.dat");
    static readonly object Lock = new();
    static AccountStore? _store;

    public static AccountStore Store
    {
        get
        {
            lock (Lock)
            {
                if (_store != null) return _store;
                try
                {
                    if (File.Exists(FilePath))
                    {
                        var raw = ProtectedData.Unprotect(File.ReadAllBytes(FilePath), null, DataProtectionScope.CurrentUser);
                        _store = JsonSerializer.Deserialize<AccountStore>(raw, Json.Options);
                    }
                }
                catch { }
                return _store ??= new AccountStore();
            }
        }
    }

    public static void Save()
    {
        lock (Lock)
        {
            var raw = JsonSerializer.SerializeToUtf8Bytes(Store, Json.Options);
            Directory.CreateDirectory(AppPaths.Root);
            File.WriteAllBytes(FilePath, ProtectedData.Protect(raw, null, DataProtectionScope.CurrentUser));
        }
    }

    public static object PublicList() => new
    {
        selected = Store.SelectedId,
        accounts = Store.Accounts.Select(a => new { id = a.Id, name = a.Name, type = a.Type }),
    };

    public static Account? Selected => Store.Accounts.FirstOrDefault(a => a.Id == Store.SelectedId) ?? Store.Accounts.FirstOrDefault();

    public static void Upsert(Account acc)
    {
        Store.Accounts.RemoveAll(a => a.Id == acc.Id);
        Store.Accounts.Add(acc);
        Store.SelectedId = acc.Id;
        Save();
    }

    public static Account AddOffline(string name)
    {
        name = name.Trim();
        if (!System.Text.RegularExpressions.Regex.IsMatch(name, "^[A-Za-z0-9_]{3,16}$"))
            throw new ArgumentException("Name: 3–16 Zeichen, nur Buchstaben, Zahlen und _");
        // Gleiche UUID wie der Vanilla-Server im Offline-Modus
        var hash = MD5.HashData(Encoding.UTF8.GetBytes("OfflinePlayer:" + name));
        hash[6] = (byte)((hash[6] & 0x0f) | 0x30);
        hash[8] = (byte)((hash[8] & 0x3f) | 0x80);
        var acc = new Account { Id = Convert.ToHexString(hash).ToLowerInvariant(), Name = name, Type = "offline", AccessToken = "0" };
        Upsert(acc);
        return acc;
    }

    public static void Remove(string id)
    {
        Store.Accounts.RemoveAll(a => a.Id == id);
        if (Store.SelectedId == id) Store.SelectedId = Store.Accounts.FirstOrDefault()?.Id;
        Save();
    }

    public static void Select(string id)
    {
        if (Store.Accounts.Any(a => a.Id == id)) { Store.SelectedId = id; Save(); }
    }
}

/// Microsoft-Login per Device-Code-Flow: Nutzer gibt einen Code auf microsoft.com/link ein,
/// danach Xbox Live -> XSTS -> Minecraft-Token.
static class MicrosoftAuth
{
    const string Scope = "XboxLive.signin offline_access";
    const string Authority = "https://login.microsoftonline.com/consumers/oauth2/v2.0";

    public record DeviceCode(string UserCode, string VerificationUri, string Message);

    public static async Task<Account> Login(string clientId, Action<DeviceCode> showCode, CancellationToken ct)
    {
        if (string.IsNullOrWhiteSpace(clientId))
            throw new InvalidOperationException("Keine Azure-Client-ID eingetragen (Einstellungen → Microsoft-Login).");

        using var dcRes = await Net.Client.PostAsync($"{Authority}/devicecode",
            new FormUrlEncodedContent(new Dictionary<string, string> { ["client_id"] = clientId, ["scope"] = Scope }), ct);
        var dc = await ReadJson(dcRes, "Device-Code");
        showCode(new DeviceCode((string)dc["user_code"]!, (string)dc["verification_uri"]!, (string?)dc["message"] ?? ""));

        var interval = (int?)dc["interval"] ?? 5;
        var deadline = DateTime.UtcNow.AddSeconds((int?)dc["expires_in"] ?? 900);
        while (DateTime.UtcNow < deadline)
        {
            await Task.Delay(TimeSpan.FromSeconds(interval), ct);
            using var res = await Net.Client.PostAsync($"{Authority}/token", new FormUrlEncodedContent(new Dictionary<string, string>
            {
                ["grant_type"] = "urn:ietf:params:oauth:grant-type:device_code",
                ["client_id"] = clientId,
                ["device_code"] = (string)dc["device_code"]!,
            }), ct);
            var body = JsonNode.Parse(await res.Content.ReadAsStringAsync(ct))!;
            var err = (string?)body["error"];
            if (err == "authorization_pending") continue;
            if (err == "slow_down") { interval += 5; continue; }
            if (err != null) throw new InvalidOperationException(err == "expired_token" ? "Code abgelaufen" : $"Microsoft-Login: {body["error_description"] ?? err}");
            return await FinishLogin((string)body["access_token"]!, (string?)body["refresh_token"], ct);
        }
        throw new TimeoutException("Login-Code abgelaufen");
    }

    /// Erneuert das Minecraft-Token, wenn es bald abläuft.
    public static async Task<Account> EnsureFresh(Account acc, string clientId, CancellationToken ct)
    {
        if (acc.Type != "msa") return acc;
        if (acc.AccessExpires - DateTimeOffset.UtcNow.ToUnixTimeSeconds() > 3600) return acc;
        if (string.IsNullOrEmpty(acc.RefreshToken)) throw new InvalidOperationException("Bitte erneut mit Microsoft anmelden");

        using var res = await Net.Client.PostAsync($"{Authority}/token", new FormUrlEncodedContent(new Dictionary<string, string>
        {
            ["grant_type"] = "refresh_token",
            ["client_id"] = clientId,
            ["refresh_token"] = acc.RefreshToken,
            ["scope"] = Scope,
        }), ct);
        var body = await ReadJson(res, "Token erneuern (bitte neu anmelden)");
        var fresh = await FinishLogin((string)body["access_token"]!, (string?)body["refresh_token"] ?? acc.RefreshToken, ct);
        Accounts.Upsert(fresh);
        return fresh;
    }

    static async Task<Account> FinishLogin(string msToken, string? refreshToken, CancellationToken ct)
    {
        var xbl = await PostJson("https://user.auth.xboxlive.com/user/authenticate", new
        {
            Properties = new { AuthMethod = "RPS", SiteName = "user.auth.xboxlive.com", RpsTicket = "d=" + msToken },
            RelyingParty = "http://auth.xboxlive.com",
            TokenType = "JWT",
        }, "Xbox Live", ct);

        using var xstsRes = await Net.Client.PostAsJsonAsync("https://xsts.auth.xboxlive.com/xsts/authorize", new
        {
            Properties = new { SandboxId = "RETAIL", UserTokens = new[] { (string)xbl["Token"]! } },
            RelyingParty = "rp://api.minecraftservices.com/",
            TokenType = "JWT",
        }, ct);
        if ((int)xstsRes.StatusCode == 401)
        {
            var e = JsonNode.Parse(await xstsRes.Content.ReadAsStringAsync(ct));
            throw new InvalidOperationException((long?)e?["XErr"] switch
            {
                2148916233 => "Dieses Microsoft-Konto hat kein Xbox-Profil. Bitte einmal auf xbox.com anmelden.",
                2148916235 => "Xbox Live ist in deinem Land nicht verfügbar.",
                2148916238 => "Kinderkonto: Ein Erwachsener muss das Konto zu einer Familie hinzufügen.",
                _ => "Xbox-Anmeldung abgelehnt",
            });
        }
        var xsts = await ReadJson(xstsRes, "XSTS");
        var uhs = (string)xsts["DisplayClaims"]!["xui"]![0]!["uhs"]!;
        var xuid = (string?)xsts["DisplayClaims"]!["xui"]![0]!["xid"];

        var mc = await PostJson("https://api.minecraftservices.com/authentication/login_with_xbox",
            new { identityToken = $"XBL3.0 x={uhs};{(string)xsts["Token"]!}" }, "Minecraft-Login", ct);
        var mcToken = (string)mc["access_token"]!;

        using var req = new HttpRequestMessage(HttpMethod.Get, "https://api.minecraftservices.com/minecraft/profile");
        req.Headers.Authorization = new("Bearer", mcToken);
        using var profRes = await Net.Client.SendAsync(req, ct);
        if (profRes.StatusCode == System.Net.HttpStatusCode.NotFound)
            throw new InvalidOperationException("Dieses Konto besitzt Minecraft: Java Edition nicht.");
        var profile = await ReadJson(profRes, "Profil");

        return new Account
        {
            Id = (string)profile["id"]!,
            Name = (string)profile["name"]!,
            Type = "msa",
            AccessToken = mcToken,
            AccessExpires = DateTimeOffset.UtcNow.ToUnixTimeSeconds() + ((long?)mc["expires_in"] ?? 86400),
            RefreshToken = refreshToken,
            Xuid = xuid,
        };
    }

    static async Task<JsonNode> PostJson(string url, object body, string what, CancellationToken ct)
    {
        using var res = await Net.Client.PostAsJsonAsync(url, body, ct);
        return await ReadJson(res, what);
    }

    static async Task<JsonNode> ReadJson(HttpResponseMessage res, string what)
    {
        var text = await res.Content.ReadAsStringAsync();
        if (!res.IsSuccessStatusCode)
        {
            var hint = res.StatusCode == System.Net.HttpStatusCode.Forbidden && what == "Minecraft-Login"
                ? " – deine Azure-App ist evtl. noch nicht für die Minecraft-API freigeschaltet."
                : "";
            throw new InvalidOperationException($"{what} fehlgeschlagen ({(int)res.StatusCode}){hint}");
        }
        return JsonNode.Parse(text)!;
    }
}
