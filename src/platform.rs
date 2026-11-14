//! Alles, was sich zwischen Windows und Linux unterscheidet: Prozesse ohne Konsolenfenster,
//! eine laufende Instanz, Einladungslinks von außen, Zwischenablage und Java-Pfade.

#[cfg(unix)]
use crate::paths;

/// Endung ausführbarer Dateien
pub const EXE: &str = if cfg!(windows) { ".exe" } else { "" };
/// Name des Betriebssystems in Mojangs Versions-JSON und Java-Laufzeiten
pub const OS_NAME: &str = if cfg!(windows) { "windows" } else { "linux" };
pub const JAVA_PLATFORM: &str = if cfg!(windows) { "windows-x64" } else { "linux" };
/// Trennzeichen im Java-Klassenpfad
pub const CLASSPATH_SEP: &str = if cfg!(windows) { ";" } else { ":" };

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Hilfsprozesse ohne aufblitzendes Konsolenfenster starten (nur unter Windows nötig)
#[cfg_attr(not(windows), allow(dead_code))]
pub fn quiet(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

pub fn quiet_async(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

// ---------- Nur eine Instanz ----------

#[cfg(windows)]
pub fn single_instance() -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = "NNNLauncher-SingleInstance\0".encode_utf16().collect();
    // SAFETY: nullterminierter Name; das Handle bleibt bis Prozessende offen (gewollt)
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        !h.is_null() && GetLastError() != ERROR_ALREADY_EXISTS
    }
}

#[cfg(unix)]
pub fn single_instance() -> bool {
    static LOCK: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();
    let _ = std::fs::create_dir_all(paths::root());
    let Ok(file) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(paths::root().join("instance.lock")) else {
        return true;
    };
    // Die Sperre hält, solange die Datei offen ist – also bis zum Programmende
    if file.try_lock().is_err() {
        return false;
    }
    let _ = LOCK.set(file);
    true
}

// ---------- Einladungslinks an die laufende Instanz ----------

#[cfg(windows)]
const PIPE: &str = r"\\.\pipe\NNNLauncher-DeepLink";

#[cfg(unix)]
fn socket_path() -> std::path::PathBuf { paths::root().join("deeplink.sock") }

/// Schickt einen Link an die schon laufende Instanz
pub fn send_link(link: &str) {
    use std::io::Write;
    #[cfg(windows)]
    let conn = std::fs::OpenOptions::new().write(true).open(PIPE);
    #[cfg(unix)]
    let conn = std::os::unix::net::UnixStream::connect(socket_path());
    if let Ok(mut c) = conn {
        let _ = writeln!(c, "{link}");
    }
}

/// Wartet dauerhaft auf Links von weiteren Starts
pub async fn listen_links(on_link: impl Fn(String) + Send + 'static) {
    use tokio::io::AsyncBufReadExt;
    loop {
        #[cfg(windows)]
        let stream = {
            let Ok(server) = tokio::net::windows::named_pipe::ServerOptions::new().create(PIPE) else {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            };
            if server.connect().await.is_err() {
                continue;
            }
            server
        };
        #[cfg(unix)]
        let stream = {
            static LISTENER: tokio::sync::OnceCell<tokio::net::UnixListener> = tokio::sync::OnceCell::const_new();
            let listener = LISTENER
                .get_or_try_init(|| async {
                    // Reste eines abgestürzten Laufs entfernen; die Instanz-Sperre stellt sicher, dass wir allein sind
                    let _ = std::fs::remove_file(socket_path());
                    tokio::net::UnixListener::bind(socket_path())
                })
                .await;
            let Ok(listener) = listener else {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            };
            let Ok((s, _)) = listener.accept().await else { continue };
            s
        };
        let mut reader = tokio::io::BufReader::new(stream);
        let mut line = String::new();
        if reader.read_line(&mut line).await.is_ok() && !line.trim().is_empty() {
            on_link(line.trim().to_string());
        }
    }
}

// ---------- Protokoll nnnlauncher:// ----------

/// nnnlauncher:// (und alte packlink://-Links) für den aktuellen Nutzer registrieren
#[cfg(windows)]
pub fn register_protocols() {
    use winreg::enums::HKEY_CURRENT_USER;
    let Ok(exe) = std::env::current_exe() else { return };
    let exe = exe.display().to_string();
    let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
    for scheme in ["nnnlauncher", "packlink"] {
        if let Ok((key, _)) = hkcu.create_subkey(format!(r"Software\Classes\{scheme}")) {
            let _ = key.set_value("", &"URL:NNN Launcher");
            let _ = key.set_value("URL Protocol", &"");
            if let Ok((icon, _)) = key.create_subkey("DefaultIcon") {
                let _ = icon.set_value("", &format!("\"{exe}\",0"));
            }
            if let Ok((cmd, _)) = key.create_subkey(r"shell\open\command") {
                let _ = cmd.set_value("", &format!("\"{exe}\" \"%1\""));
            }
        }
    }
}

/// Unter Linux: Startmenü-Eintrag mit Symbol, der auch die Links öffnet
#[cfg(unix)]
pub fn register_protocols() {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else { return };
    // In einem AppImage zeigt APPIMAGE auf die Datei, die der Nutzer gestartet hat
    let exe = std::env::var_os("APPIMAGE").map(std::path::PathBuf::from).or_else(|| std::env::current_exe().ok());
    let Some(exe) = exe else { return };
    let icon = paths::root().join("nnn-launcher.png");
    if !icon.exists() {
        let _ = std::fs::write(&icon, include_bytes!("../assets/icon.png"));
    }
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=NNN Launcher\nComment=Minecraft-Modpacks mit Freunden teilen\nExec=\"{}\" %u\nIcon={}\nTerminal=false\nCategories=Game;\nMimeType=x-scheme-handler/nnnlauncher;x-scheme-handler/packlink;\n",
        exe.display(),
        icon.display()
    );
    let apps = home.join(".local/share/applications");
    let file = apps.join("nnn-launcher.desktop");
    if std::fs::read_to_string(&file).ok().as_deref() == Some(entry.as_str()) {
        return;
    }
    if std::fs::create_dir_all(&apps).is_err() || std::fs::write(&file, entry).is_err() {
        return;
    }
    for scheme in ["nnnlauncher", "packlink"] {
        let _ = std::process::Command::new("xdg-mime").args(["default", "nnn-launcher.desktop", &format!("x-scheme-handler/{scheme}")]).status();
    }
    let _ = std::process::Command::new("update-desktop-database").arg(&apps).status();
}

// ---------- Zwischenablage ----------

#[cfg(windows)]
pub fn copy_to_clipboard(text: &str) {
    let script = format!("Set-Clipboard -Value '{}'", text.replace('\'', "''"));
    let _ = quiet(std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script])).spawn();
}

/// Wayland (wl-copy) oder X11 (xclip/xsel), je nachdem was installiert ist
#[cfg(unix)]
pub fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    let text = text.to_string();
    std::thread::spawn(move || {
        for cmd in [&["wl-copy"][..], &["xclip", "-selection", "clipboard"], &["xsel", "--clipboard", "--input"]] {
            let child = std::process::Command::new(cmd[0])
                .args(&cmd[1..])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            if let Ok(mut child) = child {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                if child.wait().is_ok_and(|s| s.success()) {
                    return;
                }
            }
        }
    });
}

/// Datei nur für den eigenen Benutzer lesbar machen (Anmeldedaten unter Linux)
pub fn make_private(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    let _ = path;
}

/// Ausführbar machen (heruntergeladenes Java, Minecraft Launcher)
pub fn make_executable(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
    }
    let _ = path;
}
