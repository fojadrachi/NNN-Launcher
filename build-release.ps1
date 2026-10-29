# Baut den NNN Launcher für ein Release:
#   dist\NNN-Launcher-Setup-<Version>.exe   (Installer, für Updates aus der App)
#   dist\NNN-Launcher-<Version>-win-x64.exe (portable, ohne Installation)
# Die Version kommt aus launcher\PackLink.csproj (<Version>).
param([string]$Iscc = "")

$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
$csproj = Join-Path $root "launcher\PackLink.csproj"
$version = ([xml](Get-Content $csproj)).Project.PropertyGroup.Version | Where-Object { $_ } | Select-Object -First 1
if (-not $version) { throw "Keine <Version> in PackLink.csproj gefunden" }
Write-Host "NNN Launcher $version" -ForegroundColor Yellow

if (-not $Iscc) {
    $Iscc = @(
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $Iscc) { throw "Inno Setup 6 nicht gefunden (winget install JRSoftware.InnoSetup)" }

$self = Join-Path $root "build\self"
if (Test-Path $self) { Remove-Item $self -Recurse -Force }
$dist = Join-Path $root "dist"
New-Item -ItemType Directory -Force $dist | Out-Null

# Eigenständig: .NET steckt in der .exe, Freunde müssen nichts installieren
dotnet publish $csproj -c Release -r win-x64 --self-contained true `
    -p:PublishSingleFile=true -p:IncludeNativeLibrariesForSelfExtract=true `
    -p:EnableCompressionInSingleFile=true -p:DebugType=none -o $self -nologo -v q
if ($LASTEXITCODE -ne 0) { throw "dotnet publish fehlgeschlagen" }
$exe = Join-Path $self "NNN Launcher.exe"

Copy-Item $exe (Join-Path $dist "NNN-Launcher-$version-win-x64.exe") -Force

& $Iscc /Q "/DAppVersion=$version" "/DSourceExe=$exe" (Join-Path $root "installer\NNN-Launcher.iss")
if ($LASTEXITCODE -ne 0) { throw "Inno Setup fehlgeschlagen" }

Get-ChildItem $dist -Filter "*$version*" | ForEach-Object {
    "{0}  {1:N1} MB" -f $_.Name, ($_.Length / 1MB)
}
