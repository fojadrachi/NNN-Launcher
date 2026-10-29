# NNN Launcher

Minecraft-Launcher mit Freunde-System und echtem Modpack-Sync.
Mods kommen von **Modrinth** und **CurseForge**, Packs werden über einen eigenen
Cloudflare-Worker geteilt.

## Was NNN Launcher anders macht

- **Freunde hinzufügen** per Benutzername, Anfragen annehmen/ablehnen
- **Pack teilen** mit einem Klick: Einladungslink (`https://…/j/CODE`) oder Code verschicken,
  der Link öffnet NNN Launcher direkt (`nnnlauncher://join/CODE`)
- **Updates pushen**: Besitzer oder Editoren klicken „Update veröffentlichen“, alle Mitglieder
  sehen innerhalb einer Minute „Update verfügbar“ und laden nur die geänderten Dateien
- Configs (`config/`, `defaultconfigs/`, `kubejs/`) können mitgeschickt werden
- Eigene Mods, die jemand selbst in den `mods`-Ordner kopiert, bleiben beim Sync unangetastet
- Schutz gegen gleichzeitiges Pushen (wer auf einer alten Version aufbaut, bekommt einen Hinweis)
- Rollen: Besitzer, Editor (darf Updates veröffentlichen), Mitglied
- Abhängigkeiten von Mods werden automatisch mitinstalliert
- Java wird automatisch passend zur Minecraft-Version geladen
- Loader: Fabric, Quilt, Forge, NeoForge, Vanilla

## Aufbau

```
packlink/
  server/     Cloudflare Worker + D1 (Konten, Freunde, Packs, Versionen, Share-Links, CurseForge-Proxy)
  launcher/   C# (.NET 8, WinForms + WebView2), Oberfläche in launcher/wwwroot (in die .exe eingebettet)
```

Daten des Launchers liegen in `%APPDATA%\NNN Launcher` (Instanzen, Minecraft-Dateien, Java, Logs).
Konten sind per Windows-DPAPI verschlüsselt.

## Server

Lokal testen:

```bash
cd server
npm install
npm run db:local
npm run dev
```

Online stellen:

```bash
npx wrangler login
npx wrangler d1 create nnn-launcher          # ID in wrangler.toml eintragen
npm run db:remote
npx wrangler secret put CF_API_KEY       # CurseForge-API-Key (bleibt geheim im Worker)
npm run deploy
```

Danach im Launcher unter **Einstellungen → NNN Launcher-Server** die Worker-Adresse eintragen
(Standard: `https://nnn-launcher.christianolah56-42e.workers.dev`).

Lokal den CurseForge-Key in `server/.dev.vars` eintragen: `CF_API_KEY=...`

## Launcher

```bash
cd launcher
dotnet build -c Release
dotnet publish -c Release -r win-x64 --self-contained false -p:PublishSingleFile=true -o ../dist
```

Selbsttest ohne Oberfläche (installiert und startet kurz Minecraft, Log in `%APPDATA%\NNN Launcher\logs\selftest.log`):

```bash
"NNN Launcher.exe" --selftest 1.20.1 fabric 0.20.1
```

## Spielstart

Standard ist der **offizielle Minecraft Launcher** (wie bei CurseForge):

1. NNN Launcher installiert Loader und Mods in `%APPDATA%\NNN Launcher\minecraft` bzw. den Instanzordner
2. schreibt dort ein Profil „NNN: &lt;Name&gt;“ in `launcher_profiles.json` (Spielordner, Version, RAM)
3. öffnet den Mojang-Launcher mit `--workDir %APPDATA%\NNN Launcher\minecraft`, das Profil ist vorausgewählt

Anmeldung und Spielstart passieren im offiziellen Launcher, eine Azure-App ist dafür nicht nötig.
Ist kein Launcher installiert, lädt NNN Launcher ihn einmalig von Mojang (`launcher.mojang.com`) und
prüft die Signatur (Mojang AB). Ein eigener Pfad lässt sich in den Einstellungen eintragen.

Alternativ kann NNN Launcher unter Einstellungen → Spielstart auch **direkt** starten (eigener Microsoft-Login, siehe unten).

## Microsoft-Login (nur für Direktstart)

Für Online-Server im Direktstart braucht NNN Launcher eine eigene Azure-App:

1. [portal.azure.com](https://portal.azure.com) → App-Registrierungen → Neu
2. Unterstützte Konten: **Nur persönliche Microsoft-Konten**
3. Authentifizierung → **Öffentliche Clientflows zulassen: Ja**
4. Client-ID im Launcher unter Einstellungen eintragen
5. Bei Mojang die Freischaltung für die Minecraft-API beantragen
   (neue Apps bekommen sonst beim Minecraft-Login einen 403-Fehler)

Bis dahin funktionieren Offline-Konten (Einzelspieler, Server mit `online-mode=false`).

## Schriften

Space Grotesk und JetBrains Mono liegen in `launcher/wwwroot/fonts` und stehen unter der
[SIL Open Font License 1.1](https://openfontlicense.org).

## Release bauen

```powershell
.\build-release.ps1
```

Baut aus der `<Version>` in `launcher/PackLink.csproj` das Setup (`dist/NNN-Launcher-Setup-<Version>.exe`, Inno Setup 6)
und die portable `.exe`. Beide als GitHub-Release mit Tag `v<Version>` hochladen – der Launcher findet das Setup
über `releases/latest` und bietet es als Update an.
