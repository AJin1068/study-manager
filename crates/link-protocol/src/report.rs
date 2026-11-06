//! What the phone reports back. Counts and ids only: no app names, no usage history.

use serde::{Deserialize, Serialize};

pub const MAX_PICKUPS_PER_REPORT: i64 = 100_000;
const SEGMENT_ID_MAX: usize = 64;

/// The phone's heartbeat. The server stamps the time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhoneBeat {
    /// The `seq` of the lease the phone currently holds (0 when it holds none).
    pub lease_seq: i64,
    pub locked: bool,
    /// Whether the blocking service is switched on. A phone that turns it off is a gap.
    pub service_enabled: bool,
}

/// How many times the screen was switched on or unlocked during one study segment. The count is
/// cumulative for the segment; a repeat of the same report is harmless.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickupReport {
    pub segment_id: String,
    pub count: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportError {
    BadSegmentId,
    BadCount,
}

impl PickupReport {
    pub fn validate(&self) -> Result<(), ReportError> {
        if self.segment_id.is_empty() || self.segment_id.len() > SEGMENT_ID_MAX {
            return Err(ReportError::BadSegmentId);
        }
        if !(0..=MAX_PICKUPS_PER_REPORT).contains(&self.count) {
            return Err(ReportError::BadCount);
        }
        Ok(())
    }
}
