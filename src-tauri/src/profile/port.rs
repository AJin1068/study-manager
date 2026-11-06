//! The port of the profile layer. The app talks to profiles only through this trait; the one real
//! adapter (`rest::RestProfile`) speaks the Supabase RPCs, and in mock mode the same adapter runs
//! on the in-memory `FakeBackend`.

use super::model::{GiftState, InboxGift, ProfileError, WireProfile};
use super::payload::{GrassEntry, SendGiftArgs, SetProfileArgs};

/// What a successful accept hands back: the item to add to the inventory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptedGift {
    pub item_id: String,
    pub sender_name: String,
}

pub trait ProfileApi: Send + Sync {
    /// Publishes the scopes and the sections they allow; returns the profile link id.
    fn set_settings(&self, args: &SetProfileArgs) -> Result<String, ProfileError>;
    fn report_grass(&self, entries: &[GrassEntry]) -> Result<(), ProfileError>;
    fn my_profile(&self) -> Result<WireProfile, ProfileError>;
    fn member_profile(&self, user: &str) -> Result<WireProfile, ProfileError>;
    fn sign_guestbook(&self, owner: &str, body: &str) -> Result<(), ProfileError>;
    fn delete_entry(&self, id: i64) -> Result<(), ProfileError>;
    fn report_entry(&self, id: i64) -> Result<(), ProfileError>;
    fn send_gift(&self, args: &SendGiftArgs) -> Result<(), ProfileError>;
    /// Whether a send with this key reached the server. `GiftState::None` is only returned as a
    /// successful answer.
    fn gift_status(&self, idem_key: &str) -> Result<GiftState, ProfileError>;
    fn gift_inbox(&self) -> Result<Vec<InboxGift>, ProfileError>;
    fn accept_gift(&self, gift_id: &str) -> Result<AcceptedGift, ProfileError>;
    fn decline_gift(&self, gift_id: &str) -> Result<(), ProfileError>;
}
