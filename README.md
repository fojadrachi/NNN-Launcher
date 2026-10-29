# NNN Launcher

Minecraft-Launcher mit Freunde-System und echtem Modpack-Sync – geschrieben in **Rust**,
Oberfläche mit [Slint](https://slint.dev). Schnell, klein (rund 20 MB) und ohne Browser-Engine.

## Funktionen

- **Mods, Texturepacks und Shader finden** auf Modrinth – Abhängigkeiten von Mods werden automatisch
  mitinstalliert, Texturepacks landen in `resourcepacks/`, Shader in `shaderpacks/`.
  CurseForge ist vorbereitet, aber ausgeschaltet (`CURSEFORGE` in `src/mods.rs`).
- **Ordner-Scan:** Dateien, die man selbst in `mods/`, `resourcepacks/` oder `shaderpacks/` legt, werden per Prüfsumme
  bei Modrinth erkannt und ins Pack übernommen; unbekannte zeigt der Launcher mit Name, Version und Icon aus der Datei.
  Von Hand gelöschte Dateien verschwinden automatisch aus der Liste.
- **Loader:** Fabric, Quilt, Forge, NeoForge und Vanilla
- **Spielstart über den offiziellen Minecraft Launcher** (wie bei CurseForge): Der NNN Launcher legt ein Profil
  „NNN: &lt;Name&gt;“ an und öffnet den Launcher von Mojang mit eigenem Arbeitsordner. Anmeldung und Spieldateien
  laufen dort. Fehlt er auf dem PC, wird er einmalig von Mojang geladen und die Signatur geprüft.
- **Freunde** per Benutzername hinzufügen
- **Modpacks teilen** per Einladungslink oder Code (`nnnlauncher://join/CODE`)
- **Updates pushen:** Besitzer oder Editoren klicken „Update veröffentlichen“, alle Mitglieder sehen
  „Update verfügbar“ und laden nur die geänderten Dateien. Eigene Mods im `mods`-Ordner bleiben erhalten.
- **Wer hat welche Version?** In der Mitgliederliste steht bei jedem die installierte Pack-Version (aktuell/veraltet)
- **Update erzwingen:** Besitzer und Editoren können die neueste Version bei allen Mitgliedern automatisch installieren
  lassen, sobald deren Launcher läuft
- **Benachrichtigungen** für Anfragen, Pack-Updates, Einladungen und neue App-Versionen
- **App-Updates** über GitHub-Releases mit Prüfsummen-Check, stille Installation und Neustart

Der Launcher spricht ausschließlich mit `https://launcher.nnn-server.de`.

## Daten

Alles liegt in `%APPDATA%\NNN Launcher`: Instanzen, Minecraft-Ordner für den offiziellen Launcher,
Java für die Forge-/NeoForge-Installer, Logs. Die Anmeldung ist per Windows-DPAPI verschlüsselt.

## Bauen

Voraussetzungen: Rust (stable, MSVC) und die Visual-C++-Build-Tools.

```powershell
cargo build --release
```

Release mit Setup (braucht zusätzlich Inno Setup 6):

```powershell
.\build-release.ps1
```

Das erzeugt `dist\NNN-Launcher-Setup-<Version>.exe` und die portable `dist\NNN-Launcher-<Version>-win-x64.exe`.
Beide als GitHub-Release mit Tag `v<Version>` hochladen – der Launcher findet das Setup über `releases/latest`.

Selbsttest ohne Oberfläche (Ergebnis in `%APPDATA%\NNN Launcher\logs\selftest.log`):

```powershell
.\target\release\nnn-launcher.exe --selftest
```

## Aufbau

```
src/          Rust-Kern: Minecraft, Loader, Mods, Pack-Sync, Server, Updates, Steuerung
ui/           Slint-Oberfläche (Seiten, Dialoge, Bausteine, Schriften)
assets/       Icon
installer/    Inno-Setup-Skript
```

## Lizenzen

Space Grotesk und JetBrains Mono (`ui/fonts`) stehen unter der
[SIL Open Font License 1.1](https://openfontlicense.org). Die Oberfläche nutzt Slint unter der
Slint Royalty-free License.
