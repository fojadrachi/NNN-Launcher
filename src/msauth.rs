//! Microsoft-Login per Gerätecode: Nutzer gibt auf microsoft.com/link einen Code ein,
//! danach Xbox Live → XSTS → Minecraft-Token → Profil.

use crate::net;
use crate::session::Account;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};

/// Azure-App des NNN Launchers (öffentliche ID, kein Geheimnis)
pub const CLIENT_ID: &str = "c6466e75-37cd-4757-ad38-912e360eacbe";
const SCOPE: &str = "XboxLive.signin offline_access";
const AUTHORITY: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0";

#[derive(Clone, Debug)]
pub struct DeviceCode {
    pub user_code: String,
    pub url: String,
}

async fn form(url: &str, params: &[(&str, &str)]) -> Result<Value> {
    let res = net::client().post(url).form(params).send().await.map_err(|_| anyhow!("Microsoft ist nicht erreichbar"))?;
    Ok(res.json().await.unwrap_or(Value::Null))
}

/// Führt den kompletten Login durch. `show` bekommt den Code für die Anzeige, `cancel` bricht ab.
pub async fn login(show: impl Fn(DeviceCode) + Send + Sync, cancel: &AtomicBool) -> Result<Account> {
    let dc = form(&format!("{AUTHORITY}/devicecode"), &[("client_id", CLIENT_ID), ("scope", SCOPE)]).await?;
    let device_code = dc["device_code"].as_str().ok_or_else(|| anyhow!("Microsoft-Login nicht möglich: {}", dc["error_description"].as_str().unwrap_or("unbekannter Fehler")))?;
    show(DeviceCode {
        user_code: dc["user_code"].as_str().unwrap_or_default().to_string(),
        url: dc["verification_uri"].as_str().unwrap_or("https://www.microsoft.com/link").to_string(),
    });

    let mut interval = dc["interval"].as_u64().unwrap_or(5);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(dc["expires_in"].as_u64().unwrap_or(900));
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        if cancel.load(Ordering::SeqCst) {
            bail!("Anmeldung abgebrochen");
        }
        let t = form(
            &format!("{AUTHORITY}/token"),
            &[("grant_type", "urn:ietf:params:oauth:grant-type:device_code"), ("client_id", CLIENT_ID), ("device_code", device_code)],
        )
        .await?;
        match t["error"].as_str() {
            Some("authorization_pending") => continue,
            Some("slow_down") => interval += 5,
            Some("expired_token") => bail!("Der Code ist abgelaufen, bitte erneut versuchen"),
            Some("authorization_declined") => bail!("Anmeldung wurde abgelehnt"),
            Some(_) => bail!("Microsoft-Login: {}", t["error_description"].as_str().unwrap_or("Fehler")),
            None => return finish(t["access_token"].as_str().unwrap_or_default(), t["refresh_token"].as_str().map(String::from)).await,
        }
    }
    bail!("Der Code ist abgelaufen, bitte erneut versuchen")
}

/// Erneuert das Minecraft-Token, wenn es bald abläuft
pub async fn ensure_fresh(acc: Account) -> Result<Account> {
    if acc.access_expires - chrono::Utc::now().timestamp() > 3600 {
        return Ok(acc);
    }
    let refresh = acc.refresh_token.clone().ok_or_else(|| anyhow!("Bitte erneut mit Microsoft anmelden"))?;
    let t = form(
        &format!("{AUTHORITY}/token"),
        &[("grant_type", "refresh_token"), ("client_id", CLIENT_ID), ("refresh_token", &refresh), ("scope", SCOPE)],
    )
    .await?;
    let token = t["access_token"].as_str().ok_or_else(|| anyhow!("Anmeldung abgelaufen – bitte erneut mit Microsoft anmelden"))?;
    finish(token, t["refresh_token"].as_str().map(String::from).or(Some(refresh))).await
}

async fn post_json(url: &str, body: Value) -> Result<reqwest::Response> {
    net::client().post(url).header("Accept", "application/json").json(&body).send().await.map_err(|_| anyhow!("Keine Verbindung zu {}", url.split('/').nth(2).unwrap_or(url)))
}

async fn finish(ms_token: &str, refresh_token: Option<String>) -> Result<Account> {
    // Xbox Live
    let xbl: Value = post_json(
        "https://user.auth.xboxlive.com/user/authenticate",
        json!({
            "Properties": { "AuthMethod": "RPS", "SiteName": "user.auth.xboxlive.com", "RpsTicket": format!("d={ms_token}") },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        }),
    )
    .await?
    .error_for_status()
    .map_err(|_| anyhow!("Xbox-Live-Anmeldung fehlgeschlagen"))?
    .json()
    .await?;

    // XSTS
    let res = post_json(
        "https://xsts.auth.xboxlive.com/xsts/authorize",
        json!({
            "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl["Token"]] },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
        }),
    )
    .await?;
    if res.status() == reqwest::StatusCode::UNAUTHORIZED {
        let e: Value = res.json().await.unwrap_or(Value::Null);
        bail!(match e["XErr"].as_u64() {
            Some(2148916233) => "Dieses Microsoft-Konto hat kein Xbox-Profil. Bitte einmal auf xbox.com anmelden.",
            Some(2148916235) => "Xbox Live ist in deinem Land nicht verfügbar.",
            Some(2148916238) => "Kinderkonto: Ein Erwachsener muss das Konto zu einer Familie hinzufügen.",
            _ => "Xbox-Anmeldung abgelehnt",
        });
    }
    let xsts: Value = res.error_for_status().map_err(|_| anyhow!("Xbox-Anmeldung fehlgeschlagen"))?.json().await?;
    let uhs = xsts["DisplayClaims"]["xui"][0]["uhs"].as_str().unwrap_or_default();
    let xuid = xsts["DisplayClaims"]["xui"][0]["xid"].as_str().map(String::from);

    // Minecraft
    let res = post_json(
        "https://api.minecraftservices.com/authentication/login_with_xbox",
        json!({ "identityToken": format!("XBL3.0 x={uhs};{}", xsts["Token"].as_str().unwrap_or_default()) }),
    )
    .await?;
    if res.status() == reqwest::StatusCode::FORBIDDEN {
        bail!("Die NNN-Launcher-App ist noch nicht für Minecraft freigeschaltet. Bitte auf die Freigabe warten und solange über den offiziellen Launcher spielen.");
    }
    let mc: Value = res.error_for_status().map_err(|_| anyhow!("Minecraft-Anmeldung fehlgeschlagen"))?.json().await?;
    let token = mc["access_token"].as_str().ok_or_else(|| anyhow!("Minecraft-Anmeldung fehlgeschlagen"))?.to_string();

    // Profil = Spielbesitz prüfen
    let res = net::client().get("https://api.minecraftservices.com/minecraft/profile").bearer_auth(&token).send().await?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        bail!("Dieses Konto besitzt Minecraft: Java Edition nicht.");
    }
    let profile: Value = res.error_for_status().map_err(|_| anyhow!("Minecraft-Profil konnte nicht geladen werden"))?.json().await?;
    Ok(Account {
        id: profile["id"].as_str().unwrap_or_default().to_string(),
        name: profile["name"].as_str().unwrap_or_default().to_string(),
        kind: "msa".into(),
        access_token: Some(token),
        access_expires: chrono::Utc::now().timestamp() + mc["expires_in"].as_i64().unwrap_or(86400),
        refresh_token,
        xuid,
    })
}
