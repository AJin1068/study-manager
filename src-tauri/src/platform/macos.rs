//! macOS power events via NSWorkspace notifications (WillSleep / DidWake).
//!
//! Permissions: none required. Observers are registered on the shared NSWorkspace
//! notification center and stay registered for the process lifetime.

use std::ptr::NonNull;
use std::sync::mpsc::Sender;

use block2::RcBlock;
use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceWillSleepNotification};
use objc2_foundation::NSNotification;

use super::{PlatformError, PowerEvent, PowerEventSource};

pub struct NsWorkspacePowerSource;

impl PowerEventSource for NsWorkspacePowerSource {
    fn start(self: Box<Self>, sender: Sender<PowerEvent>) -> Result<(), PlatformError> {
        let workspace = NSWorkspace::sharedWorkspace();
        let center = workspace.notificationCenter();

        let will_sender = sender.clone();
        let will_sleep = RcBlock::new(move |_notification: NonNull<NSNotification>| {
            // Send failure means the worker stopped; the process is shutting down.
            let _ = will_sender.send(PowerEvent::WillSleep);
        });
        let did_wake = RcBlock::new(move |_notification: NonNull<NSNotification>| {
            let _ = sender.send(PowerEvent::DidWake);
        });

        // SAFETY: the notification names are framework constants and the blocks are
        // retained by the notification center. Observer tokens are intentionally leaked:
        // they must stay registered for the whole process lifetime.
        unsafe {
            let sleep_token = center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceWillSleepNotification),
                None,
                None,
                &will_sleep,
            );
            let wake_token = center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidWakeNotification),
                None,
                None,
                &did_wake,
            );
            std::mem::forget(sleep_token);
            std::mem::forget(wake_token);
        }
        Ok(())
    }
}
