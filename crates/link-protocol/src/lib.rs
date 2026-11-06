//! The contract between the desktop app and the phone app: serde types for pairing, lock leases,
//! heartbeats and pick-up counts, and the pure state machine that turns a lease into "locked" or
//! "free" on the phone.
//!
//! Nothing here reads a clock, opens a socket or knows an app name. The phone passes in what its
//! monotonic clock measured; the wall clock is not an input at all, so changing it cannot change
//! a decision. JSON fixtures in `fixtures/phone/` (repository root) are read by the tests of this
//! crate, by the desktop app and by the phone project, so all three agree on the bytes.

mod decision;
mod lease;
mod offer;
mod report;

pub use decision::{lock_decision, FreeReason, LeaseHolder, PhoneLock};
pub use lease::{accept_lease, LeaseError, LeaseReceipt, LeaseReject, LeaseState, LockLease, ReleaseCause, GRACE_MS, MAX_LEASE_MS, MAX_SEGMENT_MS};
pub use offer::{normalize_code, OfferError, PairingOffer, CODE_LEN, OFFER_VERSION};
pub use report::{PhoneBeat, PickupReport, ReportError, MAX_PICKUPS_PER_REPORT};
