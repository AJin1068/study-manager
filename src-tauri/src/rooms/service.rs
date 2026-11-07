//! The room service: what the Tauri commands call, and the worker tick that publishes this user's
//! status and polls the rooms. Network calls always run with the database lock released.

use std::sync::{Arc, Mutex, MutexGuard};

use chrono::Local;
use rusqlite::Connection;

use super::code::{
    InvitePayload, JoinGate, decode_invite, display_code, encode_invite, normalize_code,
};
use super::feed::{FeedSource, PollingFeed};
use super::model::{
    DistractionNotice, FeedView, InviteOutcome, RoomAction, RoomCode, RoomError, RoomList,
    RoomSummary, RoomView,
};
use super::port::RoomApi;
use super::prefs::{self, FriendSettingKey, FriendSettings, LocalSnapshot};
use super::rest::RestRooms;
use super::status::{PublishOutcome, PublishSource, Publisher};
use super::together::{self, ImportResult};
use super::view::{ViewContext, build_room_view};
use super::{local, ranking};
use crate::commands::lock_db;
use crate::error::AppResult;
use crate::remote::Clock;
use crate::remote::config::RemoteConfig;
use crate::remote::payload::InventoryPublicArgs;
use crate::remote::runtime::RemoteRuntime;
use crate::remote::supabase::RemoteError;
use crate::settings::{SettingValue, UpdateOutcome};
use crate::store;

pub const INVITE_TTL_HOURS: i64 = 24;
pub const INVITE_MAX_USES: i64 = 10;

/// Reads everything the publisher and the feed need from the app.
struct AppSource {
    db: Arc<Mutex<Connection>>,
    runtime: Arc<RemoteRuntime>,
}

impl AppSource {
    fn api(&self) -> Option<Arc<dyn RoomApi>> {
        let handle = {
            let conn = lock_db(&self.db).ok()?;
            self.runtime.supabase(&conn).ok()?
        };
        if !handle.auth.status().signed_in {
            return None;
        }
        Some(Arc::new(RestRooms::new(handle.rest)))
    }
}

impl PublishSource for AppSource {
    fn api(&self) -> Option<Arc<dyn RoomApi>> {
        AppSource::api(self)
    }

    fn settings(&self) -> Option<FriendSettings> {
        FriendSettings::load(&*lock_db(&self.db).ok()?).ok()
    }

    fn in_a_room(&self) -> bool {
        lock_db(&self.db)
            .and_then(|conn| store::rooms::has_rooms(&conn))
            .unwrap_or(false)
    }

    fn local(&self, now_ms: i64) -> Option<LocalSnapshot> {
        local::collect(&*lock_db(&self.db).ok()?, &Local, now_ms).ok()
    }

    fn inventory(&self) -> Option<InventoryPublicArgs> {
        crate::shop::mirror::inventory_public(&*lock_db(&self.db).ok()?)
            .ok()
            .flatten()
    }
}

impl FeedSource for AppSource {
    fn api(&self) -> Option<Arc<dyn RoomApi>> {
        AppSource::api(self)
    }

    fn receive_enabled(&self) -> bool {
        match lock_db(&self.db) {
            Ok(conn) => FriendSettings::load(&conn)
                .map(|s| s.distraction_receive)
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    fn on_rooms(&self, rooms: &[RoomSummary], now_ms: i64) {
        let Ok(mut conn) = lock_db(&self.db) else {
            return;
        };
        if let Err(err) = store::rooms::replace_membership_cache(&mut conn, rooms, now_ms) {
            tracing::error!(error = %err, "could not cache the room list");
        }
    }
}

pub struct RoomsService {
    db: Arc<Mutex<Connection>>,
    clock: Arc<dyn Clock>,
    source: AppSource,
    publisher: Mutex<Publisher>,
    feed: PollingFeed,
    gate: Mutex<JoinGate>,
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn action(result: Result<Option<String>, RoomError>) -> AppResult<RoomAction> {
    match result {
        Ok(room_id) => Ok(RoomAction {
            code: RoomCode::Ok,
            room_id,
        }),
        Err(RoomError::Rejected(code)) => Ok(RoomAction {
            code,
            room_id: None,
        }),
        Err(other) => Err(other.into()),
    }
}

impl RoomsService {
    pub fn new(
        db: Arc<Mutex<Connection>>,
        runtime: Arc<RemoteRuntime>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let source = AppSource {
            db: Arc::clone(&db),
            runtime: Arc::clone(&runtime),
        };
        Self {
            db,
            clock,
            source,
            publisher: Mutex::new(Publisher::new()),
            feed: PollingFeed::new(),
            gate: Mutex::new(JoinGate::default()),
        }
    }

    fn api(&self) -> Result<Arc<dyn RoomApi>, RoomError> {
        AppSource::api(&self.source).ok_or(RoomError::Remote(RemoteError::NotSignedIn))
    }

    /// One worker step (called about once a second): publish, then poll. Returns the new
    /// distraction notices for the caller to show.
    pub fn tick(&self) -> Vec<DistractionNotice> {
        let Ok(now) = self.clock.now_ms() else {
            return Vec::new();
        };
        let outcome = guard(&self.publisher).tick(now, &self.source);
        if let PublishOutcome::Failed(code) = outcome {
            tracing::warn!(event = "rooms_publish_failed", code);
        }
        let result = self.feed.tick(now, &self.source);
        result
            .notices
            .into_iter()
            .filter_map(|n| {
                n.nickname
                    .filter(|name| !name.is_empty())
                    .map(|nickname| DistractionNotice {
                        nickname,
                        at_ms: n.at_ms,
                    })
            })
            .collect()
    }

    pub fn list_rooms(&self) -> AppResult<RoomList> {
        let now = self.clock.now_ms()?;
        let fresh = self.api().and_then(|api| api.list_rooms());
        match fresh {
            Ok(rooms) => {
                let mut conn = lock_db(&self.db)?;
                store::rooms::replace_membership_cache(&mut conn, &rooms, now)?;
                Ok(RoomList {
                    rooms,
                    stale: false,
                    refreshed_at_ms: Some(now),
                })
            }
            Err(_) => {
                let conn = lock_db(&self.db)?;
                let (rooms, at) = store::rooms::read_membership_cache(&conn)?;
                Ok(RoomList {
                    rooms,
                    stale: true,
                    refreshed_at_ms: at,
                })
            }
        }
    }

    fn refresh_cache(&self) {
        if let Err(err) = self.list_rooms() {
            tracing::warn!(event = "rooms_cache_refresh_failed", error = %err);
        }
    }

    pub fn create_room(&self, name: &str, nickname: &str) -> AppResult<RoomAction> {
        let now = self.clock.now_ms()?;
        let created = self
            .api()
            .and_then(|api| api.create_room(name.trim(), nickname.trim()));
        if let Ok(room_id) = &created {
            store::rooms::set_room_pref(&*lock_db(&self.db)?, room_id, nickname.trim(), now)?;
            self.refresh_cache();
        }
        action(created.map(Some))
    }

    /// `acknowledged` is the user's confirmation of the sharing notice shown before joining.
    pub fn join_room(
        &self,
        raw_code: &str,
        nickname: &str,
        acknowledged: bool,
    ) -> AppResult<RoomAction> {
        if !acknowledged {
            return Ok(RoomAction {
                code: RoomCode::InvalidInput,
                room_id: None,
            });
        }
        let Some(code) = normalize_code(raw_code) else {
            return Ok(RoomAction {
                code: RoomCode::InvalidCode,
                room_id: None,
            });
        };
        let now = self.clock.now_ms()?;
        if guard(&self.gate).try_attempt(now).is_err() {
            return Ok(RoomAction {
                code: RoomCode::RateLimited,
                room_id: None,
            });
        }
        let joined = self
            .api()
            .and_then(|api| api.join_room(&code, nickname.trim()));
        if let Ok(room_id) = &joined {
            store::rooms::set_room_pref(&*lock_db(&self.db)?, room_id, nickname.trim(), now)?;
            self.refresh_cache();
        }
        action(joined.map(Some))
    }

    pub fn leave_room(&self, room_id: &str) -> AppResult<RoomAction> {
        let left = self.api().and_then(|api| api.leave_room(room_id));
        if left.is_ok() {
            if self.feed.open_room().as_deref() == Some(room_id) {
                self.feed.set_open(None, self.clock.now_ms()?);
            }
            store::rooms::forget_room(&*lock_db(&self.db)?, room_id)?;
            self.refresh_cache();
        }
        action(left.map(|()| None))
    }

    pub fn kick_member(&self, room_id: &str, target_user: &str) -> AppResult<RoomAction> {
        action(
            self.api()
                .and_then(|api| api.kick_member(room_id, target_user))
                .map(|()| None),
        )
    }

    pub fn revoke_invite(&self, room_id: &str, invite_id: &str) -> AppResult<RoomAction> {
        action(
            self.api()
                .and_then(|api| api.revoke_invite(room_id, invite_id))
                .map(|()| None),
        )
    }

    pub fn create_invite(&self, room_id: &str) -> AppResult<InviteOutcome> {
        let none = |code| InviteOutcome {
            code,
            invite_id: None,
            invite_code: None,
            expires_at_ms: None,
            deep_link: None,
            qr_svg: None,
        };
        let made = match self
            .api()
            .and_then(|api| api.create_invite(room_id, INVITE_TTL_HOURS, INVITE_MAX_USES))
        {
            Ok(made) => made,
            Err(RoomError::Rejected(code)) => return Ok(none(code)),
            Err(other) => return Err(other.into()),
        };
        let config = RemoteConfig::load(&*lock_db(&self.db)?)?;
        let deep_link = encode_invite(
            &made.invite_code,
            &config.supabase_endpoint,
            &config.supabase_anon_key,
        );
        let qr_svg = deep_link.as_deref().and_then(crate::remote::qr::render_svg);
        Ok(InviteOutcome {
            code: RoomCode::Ok,
            invite_id: Some(made.invite_id),
            invite_code: Some(display_code(&made.invite_code)),
            expires_at_ms: Some(made.expires_at_ms),
            deep_link,
            qr_svg,
        })
    }

    /// Parses a scanned or pasted `studymanager://join?...` link. Nothing is applied.
    pub fn parse_invite(&self, text: &str) -> Option<InvitePayload> {
        decode_invite(text)
    }

    /// The room screen opened: fetches immediately so it is not empty for five seconds.
    pub fn open_room(&self, room_id: &str) -> AppResult<()> {
        let now = self.clock.now_ms()?;
        self.feed.set_open(Some(room_id.to_string()), now);
        self.feed.tick(now, &self.source);
        Ok(())
    }

    pub fn close_room(&self) -> AppResult<()> {
        self.feed.set_open(None, self.clock.now_ms()?);
        Ok(())
    }

    pub fn feed_view(&self) -> AppResult<FeedView> {
        let now = self.clock.now_ms()?;
        let state = self.feed.state();
        let room: Option<RoomView> = match &state.snapshot {
            Some((wire, fetched_at_ms)) => {
                let dates = ranking::local_dates(&Local, now)?;
                let imported =
                    store::rooms::imported_keys(&*lock_db(&self.db)?, &wire.room.room_id)?;
                let ctx = ViewContext {
                    tz: &Local,
                    dates: &dates,
                    fetched_at_ms: *fetched_at_ms,
                    now_ms: now,
                    imported: &imported,
                };
                Some(build_room_view(wire, &ctx))
            }
            None => None,
        };
        Ok(FeedView {
            room,
            degraded_code: state.degraded_code.map(str::to_string),
            last_ok_ms: state.last_ok_ms,
            poll_interval_ms: u64::try_from(state.poll_interval_ms).unwrap_or(5_000),
        })
    }

    pub fn friend_settings(&self) -> AppResult<FriendSettings> {
        FriendSettings::load(&*lock_db(&self.db)?)
    }

    pub fn update_friend_setting(
        &self,
        key: FriendSettingKey,
        value: SettingValue,
    ) -> AppResult<UpdateOutcome> {
        let now = self.clock.now_ms()?;
        prefs::update_setting(&mut *lock_db(&self.db)?, key, value, now)
    }

    /// The owner shares their next blocks with the room (an empty timetable clears it).
    pub fn share_schedule(&self, room_id: &str) -> AppResult<RoomAction> {
        let now = self.clock.now_ms()?;
        let items = together::shareable_items(&*lock_db(&self.db)?, &Local, now)?;
        action(
            self.api()
                .and_then(|api| api.share_schedule(room_id, &items))
                .map(|()| None),
        )
    }

    /// Copies the chosen blocks. The timetable is read from the server again so only what the
    /// owner shares right now can be copied.
    pub fn import_together(&self, room_id: &str, keys: &[String]) -> AppResult<Vec<ImportResult>> {
        let now = self.clock.now_ms()?;
        let snapshot = self.api().and_then(|api| api.snapshot(room_id))?;
        let Some(schedule) = snapshot.schedule else {
            return Ok(Vec::new());
        };
        let mut conn = lock_db(&self.db)?;
        together::import_items(&mut conn, &Local, room_id, &schedule, keys, now)
    }
}

/// The worker thread: one step a second. Notices are shown through `on_notice`.
pub fn spawn(service: Arc<RoomsService>, on_notice: impl Fn(DistractionNotice) + Send + 'static) {
    let spawned = std::thread::Builder::new()
        .name("rooms".into())
        .spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                for notice in service.tick() {
                    on_notice(notice);
                }
            }
        });
    if let Err(err) = spawned {
        tracing::error!(error = %err, "could not start the rooms worker");
    }
}
