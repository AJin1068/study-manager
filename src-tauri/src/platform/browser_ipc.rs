//! Local IPC between the app and the native-messaging host (a separate process Chrome starts),
//! plus the per-install token and the host manifest. Unix domain sockets on macOS; Windows named
//! pipes are not implemented yet and report `Unsupported` (verified on a real device in a later
//! stage). Nothing here touches a browser profile or the user's home on its own: the manifest
//! functions take the target directory from the caller.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::PlatformError;

/// File names inside the app data directory.
const SOCKET_FILE: &str = "browser-link.sock";
const TOKEN_FILE: &str = "browser-link.token";
const TOKEN_BYTES: usize = 32;
/// macOS `sun_path` holds 104 bytes including the terminator.
#[cfg(unix)]
const MAX_SOCKET_PATH: usize = 100;

/// Name the extension passes to `connectNative` and the manifest declares.
pub const HOST_NAME: &str = "com.studymanager.app.browser";

/// One accepted or dialed connection. Frames are read and written by the protocol layer.
pub trait LinkStream: Read + Write + Send {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    fn try_clone_stream(&self) -> io::Result<Box<dyn LinkStream>>;
}

pub trait LinkListener: Send {
    fn accept_stream(&self) -> io::Result<Box<dyn LinkStream>>;
}

/// Binds the app-side listener in `dir`, replacing a stale socket left by a crashed run (the
/// single-instance plugin guarantees no live owner).
pub fn bind_listener(dir: &Path) -> Result<Box<dyn LinkListener>, PlatformError> {
    #[cfg(unix)]
    {
        unix::bind(dir).map_err(|e| PlatformError::Ipc(e.to_string()))
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Err(PlatformError::Unsupported("browser link IPC"))
    }
}

/// Host-side dial. An error means the app is not running (or not listening).
pub fn connect(dir: &Path) -> io::Result<Box<dyn LinkStream>> {
    #[cfg(unix)]
    {
        unix::connect(dir)
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Err(io::Error::new(io::ErrorKind::Unsupported, "browser link IPC"))
    }
}

/// Reads the install token, creating it (owner-only) on first use.
pub fn ensure_token(dir: &Path) -> Result<String, PlatformError> {
    #[cfg(unix)]
    {
        unix::ensure_token(dir).map_err(|e| PlatformError::Ipc(e.to_string()))
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Err(PlatformError::Unsupported("browser link token"))
    }
}

/// Reads an existing token; the host never creates one.
pub fn read_token(dir: &Path) -> io::Result<String> {
    let raw = std::fs::read_to_string(dir.join(TOKEN_FILE))?;
    let token = raw.trim();
    if is_valid_token(token) {
        Ok(token.to_owned())
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidData, "malformed token file"))
    }
}

fn is_valid_token(token: &str) -> bool {
    token.len() == TOKEN_BYTES * 2 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The app data directory as Tauri resolves it from the bundle identifier, for the host process
/// (which has no Tauri runtime). A drift test compares `identifier` with `tauri.conf.json`.
pub fn app_data_dir(identifier: &str) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join("Library/Application Support").join(identifier))
    }
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var_os("APPDATA")?;
        Some(PathBuf::from(appdata).join(identifier))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = identifier;
        None
    }
}

// ---- native messaging host manifest ----

/// Host manifest JSON. `allowed_origins` pins the extension IDs that may start the host.
pub fn host_manifest_json(host_path: &Path, extension_ids: &[&str]) -> String {
    let origins: Vec<String> = extension_ids.iter().map(|id| format!("chrome-extension://{id}/")).collect();
    serde_json::json!({
        "name": HOST_NAME,
        "description": "Study Manager browser link",
        "path": host_path.to_string_lossy(),
        "type": "stdio",
        "allowed_origins": origins,
    })
    .to_string()
}

/// Chrome's per-user manifest directory under `home`. Other Chromium browsers have their own
/// directories; they are added once each path is confirmed on a machine that has the browser.
pub fn chrome_manifest_dir(home: &Path) -> Result<PathBuf, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        Ok(home.join("Library/Application Support/Google/Chrome/NativeMessagingHosts"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = home;
        Err(PlatformError::Unsupported("native host registration"))
    }
}

/// Writes the host manifest into `manifest_dir`. The caller chooses the directory so tests and
/// tools never write into a real browser profile by accident.
pub fn register_host(manifest_dir: &Path, host_path: &Path, extension_ids: &[&str]) -> Result<(), PlatformError> {
    let write = || -> io::Result<()> {
        std::fs::create_dir_all(manifest_dir)?;
        let file = manifest_dir.join(format!("{HOST_NAME}.json"));
        let tmp = manifest_dir.join(format!("{HOST_NAME}.json.tmp"));
        std::fs::write(&tmp, host_manifest_json(host_path, extension_ids))?;
        std::fs::rename(tmp, file)
    };
    write().map_err(|e| PlatformError::Ipc(e.to_string()))
}

/// Whether a host manifest file is present in `manifest_dir`.
pub fn is_host_registered(manifest_dir: &Path) -> bool {
    manifest_dir.join(format!("{HOST_NAME}.json")).is_file()
}

/// Removes the manifest; an absent file is already unregistered.
pub fn unregister_host(manifest_dir: &Path) -> Result<(), PlatformError> {
    match std::fs::remove_file(manifest_dir.join(format!("{HOST_NAME}.json"))) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(PlatformError::Ipc(e.to_string())),
    }
}

#[cfg(unix)]
mod unix {
    use std::io::{self, Read};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;
    use std::time::Duration;

    use super::{is_valid_token, LinkListener, LinkStream, MAX_SOCKET_PATH, SOCKET_FILE, TOKEN_BYTES, TOKEN_FILE};

    impl LinkStream for UnixStream {
        fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            UnixStream::set_read_timeout(self, timeout)
        }

        fn try_clone_stream(&self) -> io::Result<Box<dyn LinkStream>> {
            Ok(Box::new(self.try_clone()?))
        }
    }

    struct Listener(UnixListener);

    impl LinkListener for Listener {
        fn accept_stream(&self) -> io::Result<Box<dyn LinkStream>> {
            let (stream, _) = self.0.accept()?;
            Ok(Box::new(stream))
        }
    }

    pub fn bind(dir: &Path) -> io::Result<Box<dyn LinkListener>> {
        let path = dir.join(SOCKET_FILE);
        if path.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "socket path too long"));
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        Ok(Box::new(Listener(listener)))
    }

    pub fn connect(dir: &Path) -> io::Result<Box<dyn LinkStream>> {
        Ok(Box::new(UnixStream::connect(dir.join(SOCKET_FILE))?))
    }

    pub fn ensure_token(dir: &Path) -> io::Result<String> {
        if let Ok(existing) = super::read_token(dir) {
            return Ok(existing);
        }
        let mut bytes = [0u8; TOKEN_BYTES];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        debug_assert!(is_valid_token(&token));
        let path = dir.join(TOKEN_FILE);
        let tmp = dir.join(format!("{TOKEN_FILE}.tmp"));
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
            file.write_all(token.as_bytes())?;
        }
        std::fs::rename(tmp, path)?;
        Ok(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sm-ipc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn manifest_pins_extension_origins_and_host_name() {
        let json = host_manifest_json(Path::new("/opt/app/study-manager"), &["abcdefghijklmnopabcdefghijklmnop"]);
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["name"], HOST_NAME);
        assert_eq!(value["type"], "stdio");
        assert_eq!(value["path"], "/opt/app/study-manager");
        assert_eq!(value["allowed_origins"][0], "chrome-extension://abcdefghijklmnopabcdefghijklmnop/");
    }

    #[test]
    fn register_and_unregister_use_the_given_directory_only() {
        let dir = temp_dir("manifest");
        let target = dir.join("NativeMessagingHosts");
        register_host(&target, Path::new("/x/host"), &["abcdefghijklmnopabcdefghijklmnop"]).expect("register");
        assert!(target.join(format!("{HOST_NAME}.json")).exists());
        unregister_host(&target).expect("unregister");
        assert!(!target.join(format!("{HOST_NAME}.json")).exists());
        unregister_host(&target).expect("absent manifest is fine");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn data_dir_identifier_matches_tauri_conf() {
        let conf = include_str!("../../tauri.conf.json");
        let value: serde_json::Value = serde_json::from_str(conf).expect("conf json");
        assert_eq!(value["identifier"], crate::browser_link::BUNDLE_IDENTIFIER);
    }

    #[cfg(unix)]
    #[test]
    fn token_is_created_once_with_owner_only_mode_and_reread() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("token");
        let first = ensure_token(&dir).expect("create");
        assert!(is_valid_token(&first));
        assert_eq!(ensure_token(&dir).expect("reuse"), first);
        assert_eq!(read_token(&dir).expect("read"), first);
        let mode = std::fs::metadata(dir.join(TOKEN_FILE)).expect("meta").permissions().mode();
        assert_eq!(mode & 0o077, 0, "token must not be group/world readable");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn socket_round_trip_and_missing_app() {
        let dir = temp_dir("sock");
        assert!(connect(&dir).is_err(), "no listener means the app is not running");
        let listener = bind_listener(&dir).expect("bind");
        let mut client = connect(&dir).expect("connect");
        let mut server = listener.accept_stream().expect("accept");
        client.write_all(b"ping").expect("write");
        let mut buf = [0u8; 4];
        server.read_exact(&mut buf).expect("read");
        assert_eq!(&buf, b"ping");
        let _ = std::fs::remove_dir_all(dir);
    }
}
