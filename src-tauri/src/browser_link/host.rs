//! The native-messaging host: the process Chrome starts for `connectNative`. It speaks Chrome's
//! stdio framing on one side and the app's local socket on the other and relays frames in both
//! directions. It adds the install token and Chrome's caller origin as the first frame, so the
//! extension never handles the token. If the app is not running it answers with an `error` frame
//! (the extension then fails open) and exits.

use std::io::{self, Read, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use super::protocol::{encode, read_frame, write_frame, FrameError, Outgoing};
use super::BUNDLE_IDENTIFIER;
use crate::platform::{self, LinkStream};

const ORIGIN_PREFIX: &str = "chrome-extension://";

/// Chrome passes the caller's origin (`chrome-extension://<id>/`) as an argument. The app itself
/// is never started with one, so its presence selects host mode before Tauri starts.
pub fn caller_ext_id(args: &[String]) -> Option<String> {
    args.iter().find_map(|arg| {
        let id = arg.strip_prefix(ORIGIN_PREFIX)?.trim_end_matches('/');
        (!id.is_empty()).then(|| id.to_owned())
    })
}

/// Entry point for host mode. Returns the process exit code.
pub fn run(origin_ext_id: &str) -> i32 {
    let dir = platform::app_data_dir(BUNDLE_IDENTIFIER);
    let (dial, token) = match dir.as_deref() {
        Some(dir) => (platform::connect_browser_link(dir), platform::read_token(dir)),
        None => (
            Err(io::Error::new(io::ErrorKind::NotFound, "no app data directory")),
            Err(io::Error::new(io::ErrorKind::NotFound, "no app data directory")),
        ),
    };
    relay(io::stdin(), io::stdout(), dial, token, origin_ext_id)
}

fn encode_auth(token: &str, origin_ext_id: &str) -> Vec<u8> {
    // Built by hand into JSON so the host depends only on the protocol module's frame shape.
    serde_json::json!({ "type": "auth", "token": token, "originExtId": origin_ext_id }).to_string().into_bytes()
}

fn send_error<O: Write>(output: &Mutex<O>, code: &'static str) {
    if let Ok(mut out) = output.lock() {
        let _ = write_frame(&mut *out, &encode(&Outgoing::Error { code }));
    }
}

/// Relays until either side ends, then returns so the process exits: Chrome sees the port close
/// when the app goes away, which is how the extension learns the link is down.
pub fn relay<I, O>(
    mut input: I,
    output: O,
    dial: io::Result<Box<dyn LinkStream>>,
    token: io::Result<String>,
    origin_ext_id: &str,
) -> i32
where
    I: Read + Send + 'static,
    O: Write + Send + 'static,
{
    let output = Arc::new(Mutex::new(output));
    let connected = dial.and_then(|stream| token.map(|token| (stream, token))).and_then(|(mut stream, token)| {
        write_frame(&mut *stream, &encode_auth(&token, origin_ext_id))?;
        let reader = stream.try_clone_stream()?;
        Ok((stream, reader))
    });
    let (mut to_app, mut from_app) = match connected {
        Ok(pair) => pair,
        Err(_) => {
            send_error(&output, "app_not_running");
            return 1;
        }
    };

    let (done_tx, done_rx) = mpsc::channel::<()>();
    let app_done = done_tx.clone();
    let app_output = Arc::clone(&output);
    thread::spawn(move || {
        while let Ok(Some(frame)) = read_frame(&mut *from_app) {
            let Ok(mut out) = app_output.lock() else { break };
            if write_frame(&mut *out, &frame).is_err() {
                break;
            }
        }
        let _ = app_done.send(());
    });
    let browser_output = Arc::clone(&output);
    thread::spawn(move || {
        loop {
            match read_frame(&mut input) {
                Ok(Some(frame)) => {
                    if write_frame(&mut *to_app, &frame).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(FrameError::TooLarge) => {
                    send_error(&browser_output, "frame_too_large");
                    break;
                }
                Err(_) => break,
            }
        }
        let _ = done_tx.send(());
    });
    let _ = done_rx.recv();
    0
}

/// Convenience for `main`: host mode when Chrome started us.
pub fn run_if_invoked(args: &[String]) -> Option<i32> {
    caller_ext_id(args).map(|id| run(&id))
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{channel, Sender};
    use std::time::Duration;

    use super::*;

    struct ChanWriter(Sender<Vec<u8>>);

    impl Write for ChanWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let _ = self.0.send(buf.to_vec());
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn caller_origin_selects_host_mode() {
        let args = vec!["/app/study-manager".to_owned(), "chrome-extension://abcdefghijklmnopabcdefghijklmnop/".to_owned()];
        assert_eq!(caller_ext_id(&args).as_deref(), Some("abcdefghijklmnopabcdefghijklmnop"));
        assert_eq!(caller_ext_id(&["/app/study-manager".to_owned()]), None);
        assert_eq!(caller_ext_id(&["chrome-extension:///".to_owned()]), None);
    }

    #[test]
    fn app_not_running_is_reported_to_the_extension() {
        let (tx, rx) = channel();
        let code = relay(
            io::empty(),
            ChanWriter(tx),
            Err(io::Error::new(io::ErrorKind::ConnectionRefused, "no app")),
            Ok("t".repeat(64)),
            "abcdefghijklmnopabcdefghijklmnop",
        );
        assert_eq!(code, 1);
        let frame = rx.recv_timeout(Duration::from_secs(1)).expect("error frame");
        let message = read_frame(&mut io::Cursor::new(frame)).expect("frame").expect("some");
        assert_eq!(String::from_utf8(message).expect("utf8"), r#"{"type":"error","code":"app_not_running"}"#);
    }

    #[cfg(unix)]
    #[test]
    fn missing_token_counts_as_app_not_running() {
        let (tx, rx) = channel();
        let (a, _b) = std::os::unix::net::UnixStream::pair().expect("pair");
        let code = relay(
            io::empty(),
            ChanWriter(tx),
            Ok(Box::new(a)),
            Err(io::Error::new(io::ErrorKind::NotFound, "no token")),
            "x",
        );
        assert_eq!(code, 1);
        assert!(rx.recv_timeout(Duration::from_secs(1)).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn relays_both_directions_with_auth_first() {
        use std::os::unix::net::UnixStream;

        let (host_side, mut app_side) = UnixStream::pair().expect("pair");
        let (stdin_host, mut stdin_chrome) = UnixStream::pair().expect("pair");
        let (tx, rx) = channel();
        let relay_thread = thread::spawn(move || {
            relay(stdin_host, ChanWriter(tx), Ok(Box::new(host_side)), Ok("a".repeat(64)), "abcdefghijklmnopabcdefghijklmnop")
        });

        let auth = read_frame(&mut app_side).expect("auth frame").expect("some");
        let auth: serde_json::Value = serde_json::from_slice(&auth).expect("json");
        assert_eq!(auth["type"], "auth");
        assert_eq!(auth["token"], "a".repeat(64));
        assert_eq!(auth["originExtId"], "abcdefghijklmnopabcdefghijklmnop");

        write_frame(&mut stdin_chrome, br#"{"type":"hello"}"#).expect("to host");
        let relayed = read_frame(&mut app_side).expect("relayed").expect("some");
        assert_eq!(relayed, br#"{"type":"hello"}"#);

        write_frame(&mut app_side, br#"{"type":"hello_ack"}"#).expect("to chrome");
        let to_chrome = rx.recv_timeout(Duration::from_secs(2)).expect("stdout frame");
        let body = read_frame(&mut io::Cursor::new(to_chrome)).expect("frame").expect("some");
        assert_eq!(body, br#"{"type":"hello_ack"}"#);

        // The app going away ends the host, so Chrome closes the port and the extension notices.
        drop(app_side);
        assert_eq!(relay_thread.join().expect("join"), 0);
    }

    /// Spike measurement (stage 4-S0): host relay round trip, app echo included, over real
    /// Unix sockets. Not a browser measurement: Chrome's own stdio and service-worker costs are
    /// outside it.
    #[cfg(unix)]
    #[test]
    fn relay_round_trip_latency_is_small() {
        use std::os::unix::net::UnixStream;
        use std::time::Instant;

        let (host_side, mut app_side) = UnixStream::pair().expect("pair");
        let (stdin_host, mut stdin_chrome) = UnixStream::pair().expect("pair");
        let (tx, rx) = channel();
        let relay_thread =
            thread::spawn(move || relay(stdin_host, ChanWriter(tx), Ok(Box::new(host_side)), Ok("a".repeat(64)), "x"));
        let echo = thread::spawn(move || {
            let _auth = read_frame(&mut app_side);
            while let Ok(Some(frame)) = read_frame(&mut app_side) {
                if write_frame(&mut app_side, &frame).is_err() {
                    break;
                }
            }
        });
        let message = br#"{"type":"tab_event","url":"https://a.example/","tabId":1,"kind":"updated"}"#;
        let mut samples = Vec::new();
        for _ in 0..300 {
            let start = Instant::now();
            write_frame(&mut stdin_chrome, message).expect("send");
            rx.recv_timeout(Duration::from_secs(2)).expect("echo");
            samples.push(start.elapsed());
        }
        samples.sort();
        let (median, p99) = (samples[samples.len() / 2], samples[samples.len() * 99 / 100]);
        eprintln!("spike: relay round trip median={median:?} p99={p99:?}");
        assert!(median < Duration::from_millis(5), "median {median:?}");
        drop(stdin_chrome);
        assert_eq!(relay_thread.join().expect("join"), 0);
        // The echo thread ends with the process: the relay's detached reader still holds the socket.
        drop(echo);
    }
}
