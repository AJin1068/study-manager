//! The two ports of the phone link. The desktop app talks to the pairing server only through
//! `PairingApi`; the phone app through `PhoneLink`. The one real adapter of each speaks the
//! Supabase RPCs (`rest`), and mock mode runs the same adapters on the in-memory `FakeBackend`.

use link_protocol::{LockLease, PhoneBeat, PickupReport};

use super::model::{ClaimedPair, LeaseFetch, NewPairCode, PhoneError, PickupRow, PublishAck, WireStatus};

/// The desktop (owner) side.
pub trait PairingApi: Send + Sync {
    /// A new one-time code (5 minutes, one use). An earlier unused code stops working.
    fn create_pair_code(&self) -> Result<NewPairCode, PhoneError>;
    /// Ends the pairing. Access of the phone stops at once, whatever token it still holds.
    fn revoke_pair(&self) -> Result<(), PhoneError>;
    fn status(&self) -> Result<WireStatus, PhoneError>;
    /// Publishes a lease. A lower or reused sequence number comes back as `PhoneError::Stale`.
    fn publish_lock(&self, lease: &LockLease) -> Result<PublishAck, PhoneError>;
    /// The pick-up reports the paired phone made (read under the owner's SELECT policy).
    fn fetch_pickups(&self) -> Result<Vec<PickupRow>, PhoneError>;
}

/// The phone side.
pub trait PhoneLink: Send + Sync {
    /// Binds this (anonymous) phone to the owner that made the code.
    fn claim(&self, code: &str) -> Result<ClaimedPair, PhoneError>;
    /// The lease in force and the server's clock reading. `NotPaired` once the pairing is revoked.
    fn fetch_lease(&self) -> Result<LeaseFetch, PhoneError>;
    fn beat(&self, beat: &PhoneBeat) -> Result<(), PhoneError>;
    fn report_pickups(&self, report: &PickupReport) -> Result<(), PhoneError>;
    /// The phone ends the pairing from its side.
    fn leave(&self) -> Result<(), PhoneError>;
}
