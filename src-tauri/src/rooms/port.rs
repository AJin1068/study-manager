//! Ports of the room layer. The app talks to rooms only through these; the one real adapter
//! (`rest::RestRooms`) speaks the Supabase RPCs, and in mock mode the same adapter runs on the
//! in-memory `FakeBackend`, so both modes exercise the same code path.

use super::model::{CreatedInvite, RoomError, RoomSummary, WireDistraction, WireSnapshot};
use crate::remote::payload::{DailyReportArgs, FriendPrefsArgs, InventoryPublicArgs, ScheduleItemRow, StatusReportArgs};

/// Create, invite, join, leave, kick. Refusals come back as `RoomError::Rejected(code)`.
pub trait RoomService: Send + Sync {
    /// Returns the new room's id.
    fn create_room(&self, name: &str, nickname: &str) -> Result<String, RoomError>;
    fn create_invite(&self, room_id: &str, ttl_hours: i64, max_uses: i64) -> Result<CreatedInvite, RoomError>;
    fn revoke_invite(&self, room_id: &str, invite_id: &str) -> Result<(), RoomError>;
    /// `code` is already normalized. Returns the joined room's id (also for `already_member`
    /// the caller receives `Rejected(AlreadyMember)` and reads the room list).
    fn join_room(&self, code: &str, nickname: &str) -> Result<String, RoomError>;
    fn leave_room(&self, room_id: &str) -> Result<(), RoomError>;
    fn kick_member(&self, room_id: &str, target_user: &str) -> Result<(), RoomError>;
    fn list_rooms(&self) -> Result<Vec<RoomSummary>, RoomError>;
}

/// What the room screen and the notification poll read.
pub trait RoomFeed: Send + Sync {
    fn snapshot(&self, room_id: &str) -> Result<WireSnapshot, RoomError>;
    /// Distraction notices from friends since `since_ms`; empty unless this user turned
    /// receiving on.
    fn poll_distractions(&self, since_ms: i64) -> Result<Vec<WireDistraction>, RoomError>;
}

/// What this PC writes about its own user. Only this user's data ever goes out.
pub trait RoomReporter: Send + Sync {
    fn set_prefs(&self, prefs: &FriendPrefsArgs) -> Result<(), RoomError>;
    fn report_status(&self, report: &StatusReportArgs) -> Result<(), RoomError>;
    fn report_daily(&self, report: &DailyReportArgs) -> Result<(), RoomError>;
    fn report_distraction(&self) -> Result<(), RoomError>;
    fn set_inventory_public(&self, args: &InventoryPublicArgs) -> Result<(), RoomError>;
    /// The room owner's timetable; an empty list clears it.
    fn share_schedule(&self, room_id: &str, items: &[ScheduleItemRow]) -> Result<(), RoomError>;
}

/// The three faces together, so one handle serves a command or a worker.
pub trait RoomApi: RoomService + RoomFeed + RoomReporter {}
impl<T: RoomService + RoomFeed + RoomReporter> RoomApi for T {}
