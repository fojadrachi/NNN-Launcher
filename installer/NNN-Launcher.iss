; Setup für den NNN Launcher (Inno Setup 6)
; Bauen über build-release.ps1 im Hauptordner, das setzt AppVersion und den Pfad zur .exe.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\build\self\NNN Launcher.exe"
#endif

[Setup]
; AppId nie ändern – daran erkennt Windows Updates derselben App
AppId={{4FDD8CE9-8E32-440E-991F-9972DDD74388}
AppName=NNN Launcher
AppVersion={#AppVersion}
AppVerName=NNN Launcher {#AppVersion}
AppPublisher=Fojadrachi
AppPublisherURL=https://github.com/fojadrachi/NNN-Launcher
AppSupportURL=https://github.com/fojadrachi/NNN-Launcher/issues
AppUpdatesURL=https://github.com/fojadrachi/NNN-Launcher/releases
; Installation pro Nutzer, keine Adminrechte nötig
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\NNN Launcher
DisableDirPage=auto
DisableProgramGroupPage=yes
DisableWelcomePage=no
UsePreviousAppDir=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\dist
OutputBaseFilename=NNN-Launcher-Setup-{#AppVersion}
SetupIconFile=..\launcher\icon.ico
UninstallDisplayIcon={app}\NNN Launcher.exe
UninstallDisplayName=NNN Launcher
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
; Läuft der Launcher noch, wird er für das Update geschlossen
CloseApplications=force
RestartApplications=no
VersionInfoVersion={#AppVersion}
VersionInfoProductName=NNN Launcher

[Languages]
Name: "de"; MessagesFile: "compiler:Languages\German.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "NNN Launcher.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\NNN Launcher"; Filename: "{app}\NNN Launcher.exe"
Name: "{autodesktop}\NNN Launcher"; Filename: "{app}\NNN Launcher.exe"; Tasks: desktopicon

[Registry]
; Die App trägt die Link-Protokolle beim Start selbst ein; hier nur fürs saubere Entfernen
Root: HKCU; Subkey: "Software\Classes\nnnlauncher"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\packlink"; Flags: uninsdeletekey

[Run]
; Auch nach einem stillen Update (aus der App heraus) wieder starten
Filename: "{app}\NNN Launcher.exe"; Description: "{cm:LaunchProgram,NNN Launcher}"; Flags: nowait postinstall

[UninstallRun]
Filename: "{cmd}"; Parameters: "/C taskkill /IM ""NNN Launcher.exe"" /F"; Flags: runhidden; RunOnceId: "KillLauncher"

[Code]
// Beim Deinstallieren fragen, ob Instanzen und Minecraft-Dateien mit weg sollen
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir: String;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    DataDir := ExpandConstant('{userappdata}\NNN Launcher');
    if DirExists(DataDir) and not UninstallSilent then
      if MsgBox('Sollen auch deine Instanzen, Welten und heruntergeladenen Minecraft-Dateien gelöscht werden?'#13#10#13#10 + DataDir,
                mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
        DelTree(DataDir, True, True, True);
  end;
end;
