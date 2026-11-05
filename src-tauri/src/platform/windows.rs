//! Windows power events. Modern Standby (S0 low-power) wake/sleep messages are unverified on
//! real hardware, so this source reports `Unsupported` until S7a passes. The core then
//! relies on heartbeat-gap detection, and the status is shown in the UI as "검증 안 됨".

use std::sync::mpsc::Sender;

use super::{PlatformError, PowerEvent, PowerEventSource};

pub struct WindowsPowerSource;

impl PowerEventSource for WindowsPowerSource {
    fn start(self: Box<Self>, _sender: Sender<PowerEvent>) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported(
            "Windows power events pending S7a real-device verification; heartbeat fallback only",
        ))
    }
}
