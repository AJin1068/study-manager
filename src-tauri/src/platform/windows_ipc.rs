//! Windows side of the browser link: a named pipe between the app and the native-messaging host
//! (Chrome starts the host as a separate process), the install token's random source, and the
//! HKCU registry entry Chrome reads to find the host manifest.
//!
//! The pipe is overlapped so one handle can be read and written from different threads: the
//! relay reads in one thread while writing in another, which a synchronous pipe handle
//! serialises (and can deadlock). Every instance carries an explicit DACL granting only the
//! current user SID, so another local user cannot connect; the install token is a second factor.
//! Remote clients are rejected, and the first instance is created with
//! `FILE_FLAG_FIRST_PIPE_INSTANCE` so a squatter that already owns the name makes `bind` fail.
//!
//! The pure helpers (name, SDDL) compile on every OS so they are tested everywhere.

use std::path::Path;

/// FNV-1a over the UTF-8 bytes; stable across Rust versions, unlike `DefaultHasher`.
pub fn fnv1a64(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The one canonical form of the data directory used to derive the pipe name. The host computes
/// it from `%APPDATA%` and the app from Tauri's path API; they can differ in case, separators or
/// a trailing slash, so both go through here. No filesystem access (no `canonicalize`).
pub fn pipe_dir_key(dir: &Path) -> String {
    let lossy = dir.to_string_lossy().replace('/', "\\");
    let without_prefix = lossy.strip_prefix("\\\\?\\").unwrap_or(&lossy);
    without_prefix.trim_end_matches('\\').to_lowercase()
}

pub fn pipe_name(dir: &Path) -> String {
    format!(
        "\\\\.\\pipe\\study-manager-link-{:016x}",
        fnv1a64(&pipe_dir_key(dir))
    )
}

/// Protected DACL with a single allow-all ACE for the given user SID: no inherited entries and
/// no Owner Rights SID, so it holds when the app is elevated and the host is not.
pub fn owner_only_sddl(user_sid: &str) -> String {
    format!("D:P(A;;GA;;;{user_sid})")
}

#[cfg(windows)]
pub use imp::{
    bind, connect, ensure_token, registry_delete, registry_get_default, registry_set_default,
};

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::io::{self, Read, Write};
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_PIPE_BUSY,
        ERROR_PIPE_CONNECTED, FALSE, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
        INVALID_HANDLE_VALUE, LocalFree, TRUE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    };
    use windows_sys::Win32::Security::Cryptography::{
        BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
        PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_ANONYMOUS, SECURITY_SQOS_PRESENT, WriteFile,
    };
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, GetCurrentProcess, INFINITE, OpenProcessToken, WaitForSingleObject,
    };
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    use super::super::browser_ipc::{
        LinkListener, LinkStream, TOKEN_BYTES, TOKEN_FILE, is_valid_token, read_token,
    };
    use super::{owner_only_sddl, pipe_name};

    const NO_TIMEOUT: u32 = u32::MAX;
    const PIPE_BUFFER: u32 = 64 * 1024;
    /// How long a client waits for a busy pipe (all instances connected) before giving up.
    const BUSY_WAIT_MS: u32 = 2000;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn last_error() -> io::Error {
        io::Error::last_os_error()
    }

    // ---- security descriptor ----

    /// Frees the descriptor allocated by the SDDL conversion.
    struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

    impl Drop for SecurityDescriptor {
        fn drop(&mut self) {
            // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW (LocalAlloc).
            unsafe { LocalFree(self.0) };
        }
    }

    fn current_user_sid() -> io::Result<String> {
        let mut token: HANDLE = null_mut();
        // SAFETY: valid out pointer; the pseudo-handle needs no close.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(last_error());
        }
        struct Token(HANDLE);
        impl Drop for Token {
            fn drop(&mut self) {
                // SAFETY: opened above.
                unsafe { CloseHandle(self.0) };
            }
        }
        let token = Token(token);
        let mut needed = 0u32;
        // SAFETY: size probe; failure with a required size is expected.
        unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut needed) };
        if needed == 0 {
            return Err(last_error());
        }
        // u64 elements keep the buffer aligned for TOKEN_USER.
        let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
        // SAFETY: `buf` is at least `needed` bytes.
        if unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                buf.as_mut_ptr().cast::<c_void>(),
                needed,
                &mut needed,
            )
        } == 0
        {
            return Err(last_error());
        }
        // SAFETY: the buffer holds a TOKEN_USER written by the call above.
        let sid = unsafe { (*(buf.as_ptr().cast::<TOKEN_USER>())).User.Sid };
        let mut text: *mut u16 = null_mut();
        // SAFETY: `sid` points into `buf`, alive here.
        if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
            return Err(last_error());
        }
        // SAFETY: `text` is a NUL-terminated string allocated by the call; freed right after copy.
        let result = unsafe {
            let mut len = 0usize;
            while *text.add(len) != 0 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text.cast::<c_void>());
            s
        };
        Ok(result)
    }

    fn owner_descriptor() -> io::Result<SecurityDescriptor> {
        let sddl = wide(&owner_only_sddl(&current_user_sid()?));
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: valid NUL-terminated SDDL and out pointer; revision 1 is SDDL_REVISION_1.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut sd,
                null_mut(),
            )
        } == 0
        {
            return Err(last_error());
        }
        Ok(SecurityDescriptor(sd))
    }

    // ---- pipe handle and overlapped I/O ----

    struct Shared {
        handle: HANDLE,
        read_timeout_ms: AtomicU32,
    }

    // SAFETY: a pipe HANDLE may be used from any thread; operations use their own OVERLAPPED.
    unsafe impl Send for Shared {}
    unsafe impl Sync for Shared {}

    impl Drop for Shared {
        fn drop(&mut self) {
            // SAFETY: owned handle, closed once when the last clone goes away.
            unsafe { CloseHandle(self.handle) };
        }
    }

    /// Runs one overlapped operation to completion. `start` issues the call with the supplied
    /// OVERLAPPED and returns its BOOL. On timeout only this operation is cancelled
    /// (`CancelIoEx` with its own OVERLAPPED) and awaited, so a concurrent operation on the same
    /// handle is untouched and the buffer is never freed while the kernel may still write it.
    fn run_overlapped(
        handle: HANDLE,
        timeout_ms: u32,
        start: impl FnOnce(*mut OVERLAPPED) -> i32,
    ) -> io::Result<u32> {
        // SAFETY: manual-reset, initially unsignalled, unnamed event.
        let event = unsafe { CreateEventW(null(), TRUE, FALSE, null()) };
        if event.is_null() {
            return Err(last_error());
        }
        struct Event(HANDLE);
        impl Drop for Event {
            fn drop(&mut self) {
                // SAFETY: created above.
                unsafe { CloseHandle(self.0) };
            }
        }
        let event = Event(event);
        // SAFETY: OVERLAPPED is plain data; all-zero is its documented initial state.
        let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
        ov.hEvent = event.0;
        let ok = start(&mut ov);
        if ok == 0 {
            let code = unsafe { GetLastError() };
            if code != ERROR_IO_PENDING {
                return Err(io::Error::from_raw_os_error(code as i32));
            }
            let wait = if timeout_ms == NO_TIMEOUT {
                INFINITE
            } else {
                timeout_ms
            };
            // SAFETY: valid event handle.
            if unsafe { WaitForSingleObject(event.0, wait) } != WAIT_OBJECT_0 {
                // SAFETY: cancels only the operation tied to `ov`, then waits for it to finish.
                unsafe { CancelIoEx(handle, &ov) };
                let mut done = 0u32;
                // SAFETY: waits until the (cancelled or raced-to-complete) operation is over.
                if unsafe { GetOverlappedResult(handle, &ov, &mut done, TRUE) } != 0 {
                    return Ok(done);
                }
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "pipe operation timed out",
                ));
            }
        }
        let mut done = 0u32;
        // SAFETY: the operation has completed (or completed synchronously).
        if unsafe { GetOverlappedResult(handle, &ov, &mut done, TRUE) } == 0 {
            return Err(last_error());
        }
        Ok(done)
    }

    /// One end of a connected pipe. Clones share the handle and the read timeout, like a dup'd
    /// socket.
    #[derive(Clone)]
    pub struct PipeStream(Arc<Shared>);

    impl PipeStream {
        fn new(handle: HANDLE) -> Self {
            Self(Arc::new(Shared {
                handle,
                read_timeout_ms: AtomicU32::new(NO_TIMEOUT),
            }))
        }
    }

    impl Read for PipeStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let len = buf.len().min(u32::MAX as usize) as u32;
            let ptr = buf.as_mut_ptr();
            let timeout = self.0.read_timeout_ms.load(Ordering::SeqCst);
            let handle = self.0.handle;
            // SAFETY (closure): `ptr`/`len` describe `buf`, which outlives the awaited operation.
            let result = run_overlapped(handle, timeout, |ov| unsafe {
                ReadFile(handle, ptr.cast(), len, null_mut(), ov)
            });
            match result {
                Ok(n) => Ok(n as usize),
                Err(e) if e.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) => Ok(0),
                Err(e) => Err(e),
            }
        }
    }

    impl Write for PipeStream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let len = buf.len().min(u32::MAX as usize) as u32;
            let ptr = buf.as_ptr();
            let handle = self.0.handle;
            // SAFETY (closure): `ptr`/`len` describe `buf`, which outlives the awaited operation.
            let n = run_overlapped(handle, NO_TIMEOUT, |ov| unsafe {
                WriteFile(handle, ptr.cast(), len, null_mut(), ov)
            })?;
            Ok(n as usize)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl LinkStream for PipeStream {
        fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            let ms = match timeout {
                None => NO_TIMEOUT,
                Some(d) => u32::try_from(d.as_millis().max(1)).unwrap_or(NO_TIMEOUT - 1),
            };
            self.0.read_timeout_ms.store(ms, Ordering::SeqCst);
            Ok(())
        }

        fn try_clone_stream(&self) -> io::Result<Box<dyn LinkStream>> {
            Ok(Box::new(self.clone()))
        }
    }

    /// The raw handle behind a stream returned by this module, for the DACL test only.
    #[cfg(test)]
    pub fn raw_handle(stream: &dyn LinkStream) -> HANDLE {
        // SAFETY: tests only pass streams produced by `accept_stream`/`connect`, which are
        // `PipeStream`s; the cast reads the Arc pointer of that concrete type.
        let concrete = unsafe { &*(stream as *const dyn LinkStream as *const PipeStream) };
        concrete.0.handle
    }

    // ---- server ----

    struct Instance(HANDLE);

    // SAFETY: see `Shared`.
    unsafe impl Send for Instance {}

    impl Instance {
        fn into_handle(self) -> HANDLE {
            let handle = self.0;
            std::mem::forget(self);
            handle
        }
    }

    impl Drop for Instance {
        fn drop(&mut self) {
            // SAFETY: owned handle.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn create_instance(name: &[u16], first: bool) -> io::Result<Instance> {
        let sd = owner_descriptor()?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: FALSE,
        };
        let mut open_mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
        if first {
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        // SAFETY: `name` is NUL-terminated; `attributes` and its descriptor outlive the call.
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                PIPE_BUFFER,
                PIPE_BUFFER,
                0,
                &attributes,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(last_error());
        }
        Ok(Instance(handle))
    }

    struct Listener {
        name: Vec<u16>,
        next: Mutex<Option<Instance>>,
    }

    impl LinkListener for Listener {
        fn accept_stream(&self) -> io::Result<Box<dyn LinkStream>> {
            let mut slot = self
                .next
                .lock()
                .map_err(|_| io::Error::other("listener lock poisoned"))?;
            let instance = match slot.take() {
                Some(instance) => instance,
                None => create_instance(&self.name, false)?,
            };
            let handle = instance.0;
            let connect = run_overlapped(handle, NO_TIMEOUT, |ov| {
                // SAFETY: valid pipe handle and OVERLAPPED.
                unsafe { ConnectNamedPipe(handle, ov) }
            });
            match connect {
                Ok(_) => {}
                // The client connected between instance creation and this call.
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32) => {}
                Err(e) => return Err(e),
            }
            // Prepare the next listener instance before handing this one out, so a second client
            // never finds the pipe missing.
            *slot = create_instance(&self.name, false).ok();
            Ok(Box::new(PipeStream::new(instance.into_handle())))
        }
    }

    pub fn bind(dir: &Path) -> io::Result<Box<dyn LinkListener>> {
        let name = wide(&pipe_name(dir));
        let first = create_instance(&name, true)?;
        Ok(Box::new(Listener {
            name,
            next: Mutex::new(Some(first)),
        }))
    }

    // ---- client ----

    pub fn connect(dir: &Path) -> io::Result<Box<dyn LinkStream>> {
        let name = wide(&pipe_name(dir));
        for _ in 0..5 {
            // SAFETY: NUL-terminated name; no security attributes or template.
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_ANONYMOUS,
                    null_mut(),
                )
            };
            if handle != INVALID_HANDLE_VALUE {
                return Ok(Box::new(PipeStream::new(handle)));
            }
            match unsafe { GetLastError() } {
                ERROR_PIPE_BUSY => {
                    // SAFETY: NUL-terminated name.
                    unsafe { WaitNamedPipeW(name.as_ptr(), BUSY_WAIT_MS) };
                }
                ERROR_FILE_NOT_FOUND => {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "the app is not listening",
                    ));
                }
                _ => return Err(last_error()),
            }
        }
        Err(io::Error::new(io::ErrorKind::TimedOut, "pipe stayed busy"))
    }

    // ---- token ----

    pub fn ensure_token(dir: &Path) -> io::Result<String> {
        if let Ok(existing) = read_token(dir) {
            return Ok(existing);
        }
        let mut bytes = [0u8; TOKEN_BYTES];
        // SAFETY: fills the buffer from the system RNG.
        let status = unsafe {
            BCryptGenRandom(
                null_mut(),
                bytes.as_mut_ptr(),
                bytes.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status != 0 {
            return Err(io::Error::other(format!(
                "random source failed: {status:#x}"
            )));
        }
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        debug_assert!(is_valid_token(&token));
        std::fs::create_dir_all(dir)?;
        let path = dir.join(TOKEN_FILE);
        let tmp = dir.join(format!("{TOKEN_FILE}.tmp"));
        std::fs::write(&tmp, token.as_bytes())?;
        std::fs::rename(tmp, path)?;
        Ok(token)
    }

    // ---- registry (HKCU) ----

    pub fn registry_set_default(key: &str, value: &str) -> io::Result<()> {
        let (subkey, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(key)?;
        subkey.set_value("", &value)
    }

    pub fn registry_get_default(key: &str) -> Option<String> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(key)
            .ok()?
            .get_value::<String, _>("")
            .ok()
    }

    /// An absent key is already deleted.
    pub fn registry_delete(key: &str) -> io::Result<()> {
        match RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(key) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn fnv_has_a_fixed_vector() {
        assert_eq!(fnv1a64(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64("a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn dir_variants_give_one_pipe_name() {
        let a = pipe_name(Path::new(
            r"C:\Users\Me\AppData\Roaming\com.studymanager.app",
        ));
        let b = pipe_name(Path::new(
            "c:/users/me/appdata/roaming/com.studymanager.app/",
        ));
        let c = pipe_name(&PathBuf::from(
            r"\\?\C:\Users\ME\AppData\Roaming\com.studymanager.app\",
        ));
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert!(a.starts_with(r"\\.\pipe\study-manager-link-"));
        assert_ne!(
            a,
            pipe_name(Path::new(r"C:\Users\Other\com.studymanager.app"))
        );
    }

    #[cfg(windows)]
    mod live {
        use std::io::{Read, Write};
        use std::path::PathBuf;
        use std::time::{Duration, Instant};

        use super::super::imp::{bind, connect};

        fn dir(tag: &str) -> PathBuf {
            std::env::temp_dir().join(format!("sm-pipe-{tag}-{}", std::process::id()))
        }

        #[test]
        fn missing_app_is_not_found_and_second_bind_is_refused() {
            let d = dir("squat");
            let err = connect(&d).err().expect("no listener");
            assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
            let _first = bind(&d).expect("bind");
            assert!(
                bind(&d).is_err(),
                "FIRST_PIPE_INSTANCE must refuse a second owner"
            );
        }

        #[test]
        fn round_trip_eof_and_second_client() {
            let d = dir("rt");
            let listener = bind(&d).expect("bind");
            let mut client = connect(&d).expect("connect");
            let mut server = listener.accept_stream().expect("accept");
            client.write_all(b"ping").expect("write");
            let mut buf = [0u8; 4];
            server.read_exact(&mut buf).expect("read");
            assert_eq!(&buf, b"ping");
            server.write_all(b"pong").expect("reply");
            client.read_exact(&mut buf).expect("read reply");
            assert_eq!(&buf, b"pong");

            // A second client connects while the first is still open.
            let mut second = connect(&d).expect("second client");
            let mut second_server = listener.accept_stream().expect("second accept");
            second.write_all(b"two!").expect("write");
            second_server.read_exact(&mut buf).expect("read");
            assert_eq!(&buf, b"two!");

            // The peer going away is EOF, not an error.
            drop(client);
            assert_eq!(server.read(&mut buf).expect("eof"), 0);
        }

        #[test]
        fn read_timeout_cancels_only_the_read_and_a_concurrent_write_survives() {
            let d = dir("timeout");
            let listener = bind(&d).expect("bind");
            let client = connect(&d).expect("connect");
            let mut server = listener.accept_stream().expect("accept");
            server
                .set_read_timeout(Some(Duration::from_millis(150)))
                .expect("timeout");
            let mut writer = server.try_clone_stream().expect("clone");
            let mut peer = client;
            let write_thread = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                writer
                    .write_all(b"during-read")
                    .expect("write while a read is pending");
            });
            let started = Instant::now();
            let mut buf = [0u8; 8];
            let err = server.read(&mut buf).expect_err("nothing was sent");
            assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
            assert!(started.elapsed() < Duration::from_secs(2));
            write_thread.join().expect("writer");
            let mut got = [0u8; 11];
            peer.read_exact(&mut got)
                .expect("peer got the concurrent write");
            assert_eq!(&got, b"during-read");
            // The cancelled read left the stream usable.
            server.set_read_timeout(None).expect("clear timeout");
            peer.write_all(b"after").expect("after");
            let mut after = [0u8; 5];
            server.read_exact(&mut after).expect("read after cancel");
            assert_eq!(&after, b"after");
        }

        #[test]
        fn pipe_dacl_names_only_the_current_user() {
            use windows_sys::Win32::Foundation::LocalFree;
            use windows_sys::Win32::Security::Authorization::{
                ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo,
                SE_KERNEL_OBJECT,
            };
            use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

            let d = dir("dacl");
            let listener = bind(&d).expect("bind");
            // The client end carries no DACL of its own; read the descriptor of a server stream.
            let _client = connect(&d).expect("connect");
            let server = listener.accept_stream().expect("accept");
            let handle = server_handle(&*server);
            let mut sd = std::ptr::null_mut();
            // SAFETY: valid pipe handle; out pointers are valid.
            let status = unsafe {
                GetSecurityInfo(
                    handle,
                    SE_KERNEL_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut sd,
                )
            };
            assert_eq!(status, 0);
            let mut text: *mut u16 = std::ptr::null_mut();
            // SAFETY: `sd` came from GetSecurityInfo.
            let ok = unsafe {
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    sd,
                    1,
                    DACL_SECURITY_INFORMATION,
                    &mut text,
                    std::ptr::null_mut(),
                )
            };
            assert_ne!(ok, 0);
            // SAFETY: NUL-terminated string allocated by the call.
            let sddl = unsafe {
                let mut len = 0;
                while *text.add(len) != 0 {
                    len += 1;
                }
                let s = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
                LocalFree(text.cast());
                LocalFree(sd);
                s
            };
            assert!(
                sddl.starts_with("D:P(A;;FA;;;S-1-5-21-"),
                "unexpected DACL: {sddl}"
            );
            assert_eq!(sddl.matches("(A;").count(), 1, "exactly one ACE: {sddl}");
        }

        fn server_handle(
            stream: &dyn super::super::super::browser_ipc::LinkStream,
        ) -> *mut std::ffi::c_void {
            super::super::imp::raw_handle(stream)
        }
    }

    #[test]
    fn sddl_grants_only_the_given_sid() {
        assert_eq!(
            owner_only_sddl("S-1-5-21-1-2-3-1001"),
            "D:P(A;;GA;;;S-1-5-21-1-2-3-1001)"
        );
    }
}
