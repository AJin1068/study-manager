//! Local IPC between the app and the native-messaging host (a separate process Chrome starts),
//! plus the per-install token and the host manifest. Unix domain sockets on macOS, a per-user
//! named pipe on Windows (`windows_ipc.rs`). Nothing here touches a browser profile or the
//! user's home on its own: the registration functions take their target from the caller, so
//! tests never write into a real browser profile or the real registry key.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::PlatformError;

/// File names inside the app data directory.
#[cfg(unix)]
const SOCKET_FILE: &str = "browser-link.sock";
pub(super) const TOKEN_FILE: &str = "browser-link.token";
pub(super) const TOKEN_BYTES: usize = 32;
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
    #[cfg(windows)]
    {
        super::windows_ipc::bind(dir).map_err(|e| PlatformError::Ipc(e.to_string()))
    }
    #[cfg(not(any(unix, windows)))]
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
    #[cfg(windows)]
    {
        super::windows_ipc::connect(dir)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = dir;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "browser link IPC",
        ))
    }
}

/// Reads the install token, creating it (owner-only) on first use.
pub fn ensure_token(dir: &Path) -> Result<String, PlatformError> {
    #[cfg(unix)]
    {
        unix::ensure_token(dir).map_err(|e| PlatformError::Ipc(e.to_string()))
    }
    #[cfg(windows)]
    {
        super::windows_ipc::ensure_token(dir).map_err(|e| PlatformError::Ipc(e.to_string()))
    }
    #[cfg(not(any(unix, windows)))]
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
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "malformed token file",
        ))
    }
}

pub(super) fn is_valid_token(token: &str) -> bool {
    token.len() == TOKEN_BYTES * 2 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The app data directory as Tauri resolves it from the bundle identifier, for the host process
/// (which has no Tauri runtime). A drift test compares `identifier` with `tauri.conf.json`.
pub fn app_data_dir(identifier: &str) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join("Library/Application Support")
                .join(identifier),
        )
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
    let origins: Vec<String> = extension_ids
        .iter()
        .map(|id| format!("chrome-extension://{id}/"))
        .collect();
    serde_json::json!({
        "name": HOST_NAME,
        "description": "Study Manager browser link",
        "path": host_path.to_string_lossy(),
        "type": "stdio",
        "allowed_origins": origins,
    })
    .to_string()
}

/// Where a host registration lives: the manifest file's directory, and on Windows also the HKCU
/// registry key (relative to HKCU) whose default value Chrome reads to find the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostTarget {
    pub manifest_dir: PathBuf,
    /// Always `None` off Windows. Only the Windows functions read it.
    #[cfg_attr(not(windows), allow(dead_code))]
    registry_key: Option<String>,
}

impl HostTarget {
    /// A file-only target: the macOS manifest directory, or a test directory that must not touch
    /// a browser profile or the registry.
    #[cfg(any(target_os = "macos", test))]
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            manifest_dir: dir.into(),
            registry_key: None,
        }
    }

    /// A target with an explicit registry key; tests use a throwaway key, never Chrome's.
    #[cfg(all(windows, test))]
    fn with_registry_key(dir: impl Into<PathBuf>, key: String) -> Self {
        Self {
            manifest_dir: dir.into(),
            registry_key: Some(key),
        }
    }

    fn manifest_file(&self) -> PathBuf {
        self.manifest_dir.join(format!("{HOST_NAME}.json"))
    }
}

/// Chrome's registration target. macOS: the per-user manifest directory under `home`. Windows:
/// a manifest in the app data directory plus the HKCU key Chrome follows to it. Other Chromium
/// browsers have their own locations; they are added once each is confirmed on a machine that
/// has the browser.
pub fn chrome_host_target(home: &Path, app_data_dir: &Path) -> Result<HostTarget, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        let _ = app_data_dir;
        Ok(HostTarget::in_dir(home.join(
            "Library/Application Support/Google/Chrome/NativeMessagingHosts",
        )))
    }
    #[cfg(target_os = "windows")]
    {
        let _ = home;
        Ok(HostTarget {
            manifest_dir: app_data_dir.join("native-messaging"),
            registry_key: Some(format!(
                r"Software\Google\Chrome\NativeMessagingHosts\{HOST_NAME}"
            )),
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (home, app_data_dir);
        Err(PlatformError::Unsupported("native host registration"))
    }
}

/// Writes the host manifest, then (Windows) points the registry key at it. The caller chooses the
/// target so tests and tools never write into a real browser profile or Chrome's key by accident.
pub fn register_host(
    target: &HostTarget,
    host_path: &Path,
    extension_ids: &[&str],
) -> Result<(), PlatformError> {
    let write = || -> io::Result<()> {
        std::fs::create_dir_all(&target.manifest_dir)?;
        let file = target.manifest_file();
        let tmp = target.manifest_dir.join(format!("{HOST_NAME}.json.tmp"));
        std::fs::write(&tmp, host_manifest_json(host_path, extension_ids))?;
        std::fs::rename(tmp, &file)?;
        #[cfg(windows)]
        if let Some(key) = &target.registry_key {
            super::windows_ipc::registry_set_default(key, &file.to_string_lossy())?;
        }
        Ok(())
    };
    write().map_err(|e| PlatformError::Ipc(e.to_string()))
}

/// Whether the manifest file is present and, on Windows, the registry key points at it.
pub fn is_host_registered(target: &HostTarget) -> bool {
    let file = target.manifest_file();
    if !file.is_file() {
        return false;
    }
    #[cfg(windows)]
    if let Some(key) = &target.registry_key {
        return super::windows_ipc::registry_get_default(key).as_deref()
            == Some(&*file.to_string_lossy());
    }
    true
}

/// Removes the registry key and the manifest; anything already absent is already unregistered.
pub fn unregister_host(target: &HostTarget) -> Result<(), PlatformError> {
    #[cfg(windows)]
    if let Some(key) = &target.registry_key {
        super::windows_ipc::registry_delete(key).map_err(|e| PlatformError::Ipc(e.to_string()))?;
    }
    match std::fs::remove_file(target.manifest_file()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(PlatformError::Ipc(e.to_string())),
    }
}

/// A connected stream pair for relay tests: Unix socket pair, or a real named pipe on Windows.
#[cfg(test)]
pub fn test_stream_pair() -> (Box<dyn LinkStream>, Box<dyn LinkStream>) {
    #[cfg(unix)]
    {
        let (a, b) = std::os::unix::net::UnixStream::pair().expect("pair");
        (Box::new(a), Box::new(b))
    }
    #[cfg(windows)]
    {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sm-pair-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let listener = bind_listener(&dir).expect("bind");
        let dialer = std::thread::spawn({
            let dir = dir.clone();
            move || connect(&dir).expect("connect")
        });
        let server = listener.accept_stream().expect("accept");
        (server, dialer.join().expect("dial"))
    }
}

#[cfg(unix)]
mod unix {
    use std::io::{self, Read};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;
    use std::time::Duration;

    use super::{
        LinkListener, LinkStream, MAX_SOCKET_PATH, SOCKET_FILE, TOKEN_BYTES, TOKEN_FILE,
        is_valid_token,
    };

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
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "socket path too long",
            ));
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
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
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
        let json = host_manifest_json(
            Path::new("/opt/app/study-manager"),
            &["abcdefghijklmnopabcdefghijklmnop"],
        );
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["name"], HOST_NAME);
        assert_eq!(value["type"], "stdio");
        assert_eq!(value["path"], "/opt/app/study-manager");
        assert_eq!(
            value["allowed_origins"][0],
            "chrome-extension://abcdefghijklmnopabcdefghijklmnop/"
        );
    }

    #[test]
    fn register_and_unregister_use_the_given_directory_only() {
        let dir = temp_dir("manifest");
        let target = HostTarget::in_dir(dir.join("NativeMessagingHosts"));
        register_host(
            &target,
            Path::new("/x/host"),
            &["abcdefghijklmnopabcdefghijklmnop"],
        )
        .expect("register");
        assert!(
            target
                .manifest_dir
                .join(format!("{HOST_NAME}.json"))
                .exists()
        );
        assert!(is_host_registered(&target));
        unregister_host(&target).expect("unregister");
        assert!(
            !target
                .manifest_dir
                .join(format!("{HOST_NAME}.json"))
                .exists()
        );
        unregister_host(&target).expect("absent manifest is fine");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Removes the throwaway registry tree even when an assertion fails.
    #[cfg(windows)]
    struct KeyGuard(String);

    #[cfg(windows)]
    impl Drop for KeyGuard {
        fn drop(&mut self) {
            let _ = super::super::windows_ipc::registry_delete(&self.0);
            // Also drop the shared parent when it is empty (delete_subkey fails if not).
            let _ = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
                .delete_subkey(r"Software\StudyManagerTest");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_registration_writes_the_file_and_a_throwaway_registry_key() {
        let dir = temp_dir("winreg");
        let root = format!(r"Software\StudyManagerTest\{}", std::process::id());
        let _guard = KeyGuard(root.clone());
        let key = format!(r"{root}\NativeMessagingHosts\{HOST_NAME}");
        let target = HostTarget::with_registry_key(dir.join("native-messaging"), key.clone());
        let host = Path::new(r"C:\Program Files\Study Manager\study-manager.exe");
        assert!(!is_host_registered(&target));
        register_host(&target, host, &["abcdefghijklmnopabcdefghijklmnop"]).expect("register");
        assert!(is_host_registered(&target));
        let file = target.manifest_dir.join(format!("{HOST_NAME}.json"));
        assert_eq!(
            super::super::windows_ipc::registry_get_default(&key).as_deref(),
            Some(&*file.to_string_lossy())
        );
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).expect("file")).expect("json");
        assert_eq!(manifest["path"], host.to_string_lossy().as_ref());
        // A manifest without the registry value (or the reverse) is not a working registration.
        super::super::windows_ipc::registry_delete(&key).expect("delete key");
        assert!(!is_host_registered(&target));
        register_host(&target, host, &["abcdefghijklmnopabcdefghijklmnop"]).expect("again");
        unregister_host(&target).expect("unregister");
        assert!(!is_host_registered(&target));
        assert!(!file.exists());
        unregister_host(&target).expect("absent is fine");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn windows_chrome_target_points_at_the_chrome_key_and_app_data_manifest() {
        let target = chrome_host_target(Path::new(r"C:\Users\me"), Path::new(r"C:\data\app"))
            .expect("target");
        assert_eq!(
            target.manifest_dir,
            Path::new(r"C:\data\app\native-messaging")
        );
        assert_eq!(
            target.registry_key.as_deref(),
            Some(r"Software\Google\Chrome\NativeMessagingHosts\com.studymanager.app.browser")
        );
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
        let mode = std::fs::metadata(dir.join(TOKEN_FILE))
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "token must not be group/world readable");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn token_is_created_once_and_reread_on_windows() {
        let dir = temp_dir("token");
        let first = ensure_token(&dir).expect("create");
        assert!(is_valid_token(&first));
        assert_eq!(ensure_token(&dir).expect("reuse"), first);
        assert_eq!(read_token(&dir).expect("read"), first);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn socket_round_trip_and_missing_app() {
        let dir = temp_dir("sock");
        assert!(
            connect(&dir).is_err(),
            "no listener means the app is not running"
        );
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
