//! Why the app is being asked to quit. Platform-neutral so the policy is unit-testable.
//!
//! On macOS a quit that arrives as an Apple Event carries a reason attribute (`keyReasonAttr`,
//! `'why?'`). The OS sends it with a system code when the user logs out, restarts or shuts down;
//! an `osascript ... to quit` or a Dock "Quit" has no such code. The study lock may refuse the
//! second kind but must never block the first.
//!
//! Windows: `WM_QUERYENDSESSION` handling was investigated only, not implemented. The tray quit
//! item and `app.exit` already pass the exit gate; a session-ending message cannot be refused
//! without trapping the user at logoff, so Windows installs no native quit gate (`install_quit_gate` is
//! a defined `Ok` no-op there).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitReason {
    /// Ordinary quit request (Cmd+Q path, Apple Event `quit`, Dock menu). Subject to the lock.
    Requested,
    LogOut,
    Restart,
    ShutDown,
}

impl QuitReason {
    /// OS-initiated session end: never refused, only recorded.
    pub fn is_system(self) -> bool {
        !matches!(self, QuitReason::Requested)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            QuitReason::Requested => "requested",
            QuitReason::LogOut => "logout",
            QuitReason::Restart => "restart",
            QuitReason::ShutDown => "shutdown",
        }
    }

    /// Maps the four-char code of the Apple Event reason attribute. Unknown or absent codes are
    /// an ordinary request: only codes known to be system shutdown kinds bypass the lock.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn from_apple_event_code(code: u32) -> QuitReason {
        match &code.to_be_bytes() {
            b"logo" | b"rlgo" => QuitReason::LogOut, // kAELogOut, kAEReallyLogOut
            b"rest" | b"rrst" => QuitReason::Restart, // kAERestart, kAEShowRestartDialog
            b"shut" | b"rsdn" => QuitReason::ShutDown, // kAEShutDown, kAEShowShutdownDialog
            _ => QuitReason::Requested,
        }
    }
}

/// Decides one quit request. The lock gate applies to `Requested` only; system reasons are always
/// allowed and never consult it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn quit_allowed(reason: QuitReason, gate: impl FnOnce() -> bool) -> bool {
    reason.is_system() || gate()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(s: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*s)
    }

    #[test]
    fn system_codes_map_to_system_reasons() {
        for (c, r) in [
            (b"logo", QuitReason::LogOut),
            (b"rlgo", QuitReason::LogOut),
            (b"rest", QuitReason::Restart),
            (b"rrst", QuitReason::Restart),
            (b"shut", QuitReason::ShutDown),
            (b"rsdn", QuitReason::ShutDown),
        ] {
            assert_eq!(QuitReason::from_apple_event_code(code(c)), r);
            assert!(r.is_system());
        }
    }

    #[test]
    fn plain_quit_unknown_and_absent_codes_stay_gated() {
        for c in [code(b"quit"), code(b"xxxx"), 0] {
            let r = QuitReason::from_apple_event_code(c);
            assert_eq!(r, QuitReason::Requested);
            assert!(!r.is_system());
        }
    }

    #[test]
    fn system_reason_skips_the_gate_and_requested_obeys_it() {
        let mut called = false;
        assert!(quit_allowed(QuitReason::LogOut, || {
            called = true;
            false
        }));
        assert!(!called, "the lock gate must not even run for a system quit");
        assert!(!quit_allowed(QuitReason::Requested, || false));
        assert!(quit_allowed(QuitReason::Requested, || true));
    }
}
