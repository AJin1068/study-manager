//! macOS quit gate: injects `applicationShouldTerminate:` into the tao application delegate.
//!
//! tao 0.37.2 implements `applicationWillTerminate:` but not `applicationShouldTerminate:`, so
//! NSApp `terminate:` (Apple Event `quit`, Dock "Quit") never reaches `RunEvent::ExitRequested`.
//! The method is added to the delegate's class with the objc runtime. If the class already has
//! one (a future tao), `class_addMethod` refuses and the install reports an error instead of
//! replacing it; the caller then falls back to the ExitRequested gate, and a bypassed quit
//! leaves the session without `clean_exit`, so the next launch records it as `forced_quit`.
//!
//! The reason for the quit is read from the current Apple Event's `keyReasonAttr`, so OS
//! logout/restart/shutdown are not refused (see `QuitReason`).

use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::OnceLock;

use objc2::ffi::class_addMethod;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{msg_send, sel, MainThreadMarker};
use objc2::rc::Retained;
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager};

use super::quit::{quit_allowed, QuitReason};
use super::PlatformError;

type Gate = Box<dyn Fn(QuitReason) -> bool + Send + Sync>;

static GATE: OnceLock<Gate> = OnceLock::new();

const NS_TERMINATE_CANCEL: usize = 0;
const NS_TERMINATE_NOW: usize = 1;
/// `keyReasonAttr` = 'why?'.
const KEY_REASON_ATTR: u32 = u32::from_be_bytes(*b"why?");

fn terminate_reply(allowed: bool) -> usize {
    if allowed { NS_TERMINATE_NOW } else { NS_TERMINATE_CANCEL }
}

fn current_quit_reason() -> QuitReason {
    let code = NSAppleEventManager::sharedAppleEventManager()
        .currentAppleEvent()
        // The typed binding needs the objc2-core-services crate for `AEKeyword`; a plain
        // message send is equivalent (`AEKeyword` is a `u32`).
        .and_then(|event| {
            // SAFETY: `attributeDescriptorForKeyword:` takes a u32 and returns an optional descriptor.
            let d: Option<Retained<NSAppleEventDescriptor>> =
                unsafe { msg_send![&*event, attributeDescriptorForKeyword: KEY_REASON_ATTR] };
            d
        })
        .map(|d| match d.typeCodeValue() {
            0 => d.enumCodeValue(),
            c => c,
        })
        .unwrap_or(0);
    QuitReason::from_apple_event_code(code)
}

extern "C-unwind" fn application_should_terminate(_this: *mut AnyObject, _sel: Sel, _sender: *mut AnyObject) -> usize {
    // A panic must not unwind into AppKit; an unknown outcome is refused (fail closed), the
    // same as the ExitRequested gate.
    let allowed = catch_unwind(AssertUnwindSafe(|| {
        let reason = current_quit_reason();
        match GATE.get() {
            Some(gate) => quit_allowed(reason, || gate(reason)),
            None => reason.is_system(),
        }
    }))
    .unwrap_or(false);
    terminate_reply(allowed)
}

/// Must run on the main thread after tao has set the app delegate (inside Tauri `setup`).
pub fn install_quit_gate(gate: Gate) -> Result<(), PlatformError> {
    if MainThreadMarker::new().is_none() {
        return Err(PlatformError::Process("quit gate must be installed on the main thread".into()));
    }
    let app_class = AnyClass::get(c"NSApplication")
        .ok_or_else(|| PlatformError::Process("NSApplication class not found".into()))?;
    // SAFETY: `sharedApplication` and `delegate` are plain getters, called on the main thread.
    let app: *mut AnyObject = unsafe { msg_send![app_class, sharedApplication] };
    if app.is_null() {
        return Err(PlatformError::Process("NSApplication is not available".into()));
    }
    // SAFETY: `app` is the live shared application, checked non-null above.
    let delegate: *mut AnyObject = unsafe { msg_send![app, delegate] };
    if delegate.is_null() {
        return Err(PlatformError::Process("application delegate is not set yet".into()));
    }
    // SAFETY: `delegate` is a live object, checked non-null above.
    let class = unsafe { (*delegate).class() };

    GATE.set(gate).map_err(|_| PlatformError::Process("quit gate already installed".into()))?;

    // Type encoding: returns NSUInteger ('Q'), takes self ('@'), _cmd (':'), sender ('@').
    // SAFETY: the IMP's signature matches the encoding and `applicationShouldTerminate:`.
    let imp: Imp = unsafe {
        std::mem::transmute::<
            extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize,
            Imp,
        >(application_should_terminate)
    };
    let types: *const c_char = c"Q@:@".as_ptr();
    // SAFETY: `class` is the delegate's class; `types` is NUL-terminated.
    let added = unsafe {
        class_addMethod(
            std::ptr::from_ref(class).cast_mut(),
            sel!(applicationShouldTerminate:),
            imp,
            types,
        )
    };
    if added.as_bool() {
        Ok(())
    } else {
        Err(PlatformError::Process("delegate already implements applicationShouldTerminate:".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_maps_to_appkit_constants() {
        assert_eq!(terminate_reply(true), 1, "NSTerminateNow");
        assert_eq!(terminate_reply(false), 0, "NSTerminateCancel");
    }

    #[test]
    fn without_an_apple_event_the_reason_is_an_ordinary_request() {
        assert_eq!(current_quit_reason(), QuitReason::Requested);
    }

    #[test]
    fn install_off_the_main_thread_is_refused_without_touching_the_gate() {
        // Unit tests run on worker threads, never the main thread.
        let err = install_quit_gate(Box::new(|_| true));
        assert!(err.is_err());
        assert!(GATE.get().is_none());
    }
}
