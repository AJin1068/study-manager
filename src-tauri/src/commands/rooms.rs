//! Study-room commands. Calls that reach the server block, so they run on the blocking pool with
//! the database lock released.

use std::sync::Arc;

use tauri::State;

use crate::error::{AppError, AppResult};
use crate::rooms::code::InvitePayload;
use crate::rooms::model::{FeedView, InviteOutcome, RoomAction, RoomList};
use crate::rooms::prefs::{FriendSettingKey, FriendSettings};
use crate::rooms::service::RoomsService;
use crate::rooms::together::ImportResult;
use crate::settings::{SettingValue, UpdateOutcome};

type Service<'a> = State<'a, Arc<RoomsService>>;

async fn blocking<T: Send + 'static>(
    service: &Service<'_>,
    work: impl FnOnce(&RoomsService) -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    let service = Arc::clone(service);
    tauri::async_runtime::spawn_blocking(move || work(&service))
        .await
        .map_err(|_| AppError::Internal("the room job did not finish".into()))?
}

#[tauri::command]
pub async fn rooms_list(service: Service<'_>) -> AppResult<RoomList> {
    blocking(&service, |s| s.list_rooms()).await
}

#[tauri::command]
pub async fn rooms_create(
    service: Service<'_>,
    name: String,
    nickname: String,
) -> AppResult<RoomAction> {
    blocking(&service, move |s| s.create_room(&name, &nickname)).await
}

#[tauri::command]
pub async fn rooms_join(
    service: Service<'_>,
    code: String,
    nickname: String,
    acknowledged: bool,
) -> AppResult<RoomAction> {
    blocking(&service, move |s| {
        s.join_room(&code, &nickname, acknowledged)
    })
    .await
}

#[tauri::command]
pub async fn rooms_leave(service: Service<'_>, room_id: String) -> AppResult<RoomAction> {
    blocking(&service, move |s| s.leave_room(&room_id)).await
}

#[tauri::command]
pub async fn rooms_kick(
    service: Service<'_>,
    room_id: String,
    user_id: String,
) -> AppResult<RoomAction> {
    blocking(&service, move |s| s.kick_member(&room_id, &user_id)).await
}

#[tauri::command]
pub async fn rooms_create_invite(
    service: Service<'_>,
    room_id: String,
) -> AppResult<InviteOutcome> {
    blocking(&service, move |s| s.create_invite(&room_id)).await
}

#[tauri::command]
pub async fn rooms_revoke_invite(
    service: Service<'_>,
    room_id: String,
    invite_id: String,
) -> AppResult<RoomAction> {
    blocking(&service, move |s| s.revoke_invite(&room_id, &invite_id)).await
}

#[tauri::command]
pub fn rooms_parse_invite(service: Service<'_>, text: String) -> Option<InvitePayload> {
    service.parse_invite(&text)
}

#[tauri::command]
pub async fn rooms_open(service: Service<'_>, room_id: String) -> AppResult<()> {
    blocking(&service, move |s| s.open_room(&room_id)).await
}

#[tauri::command]
pub fn rooms_close(service: Service<'_>) -> AppResult<()> {
    service.close_room()
}

#[tauri::command]
pub fn rooms_feed(service: Service<'_>) -> AppResult<FeedView> {
    service.feed_view()
}

#[tauri::command]
pub fn rooms_friend_settings(service: Service<'_>) -> AppResult<FriendSettings> {
    service.friend_settings()
}

#[tauri::command]
pub fn rooms_update_friend_setting(
    service: Service<'_>,
    key: FriendSettingKey,
    value: SettingValue,
) -> AppResult<UpdateOutcome> {
    service.update_friend_setting(key, value)
}

#[tauri::command]
pub async fn rooms_share_schedule(service: Service<'_>, room_id: String) -> AppResult<RoomAction> {
    blocking(&service, move |s| s.share_schedule(&room_id)).await
}

#[tauri::command]
pub async fn rooms_import_together(
    service: Service<'_>,
    room_id: String,
    keys: Vec<String>,
) -> AppResult<Vec<ImportResult>> {
    blocking(&service, move |s| s.import_together(&room_id, &keys)).await
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use crate::rooms::model::*;
    use crate::rooms::prefs::FriendSettings;
    use crate::rooms::together::{ImportOutcome, ImportResult};

    fn fixture() -> Value {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../src/api/fixtures/rooms-contract.json"
        );
        serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json")
    }

    fn json<T: serde::Serialize>(value: &T) -> Value {
        serde_json::to_value(value).expect("serialize")
    }

    #[test]
    fn room_wire_shapes_match_the_shared_fixture() {
        let f = fixture();
        let summary = RoomSummary {
            room_id: "r1".into(),
            name: "스터디".into(),
            role: RoomRole::Owner,
            member_count: 2,
            capacity: 8,
        };
        assert_eq!(
            json(&RoomList {
                rooms: vec![summary],
                stale: false,
                refreshed_at_ms: Some(10)
            }),
            f["list"]
        );
        assert_eq!(
            json(&RoomAction {
                code: RoomCode::Ok,
                room_id: Some("r1".into())
            }),
            f["action"]
        );
        assert_eq!(
            json(&InviteOutcome {
                code: RoomCode::Ok,
                invite_id: Some("i1".into()),
                invite_code: Some("ABCDE-FGHJK".into()),
                expires_at_ms: Some(86_400_000),
                deep_link: Some("studymanager://join?c=ABCDEFGHJK".into()),
                qr_svg: Some("<svg></svg>".into()),
            }),
            f["invite"]
        );
        let me = MemberView {
            user_id: "u1".into(),
            nickname: "가".into(),
            role: RoomRole::Owner,
            is_self: true,
            level: Some(4),
            presence: PresenceView {
                kind: PresenceKind::Studying,
                remaining_sec: Some(600),
            },
            attempts_today: None,
            character: Some(CharacterView {
                species: "cat".into(),
                hat: None,
                top: None,
                accessory: None,
                hand: None,
            }),
        };
        let hidden = MemberView {
            user_id: "u2".into(),
            nickname: "나".into(),
            role: RoomRole::Member,
            is_self: false,
            level: None,
            presence: PresenceView {
                kind: PresenceKind::Hidden,
                remaining_sec: None,
            },
            attempts_today: None,
            character: None,
        };
        let ranking = |minutes: i64| {
            vec![
                RankRow {
                    user_id: "u1".into(),
                    nickname: "가".into(),
                    is_self: true,
                    minutes: Some(minutes),
                    rank: Some(1),
                },
                RankRow {
                    user_id: "u2".into(),
                    nickname: "나".into(),
                    is_self: false,
                    minutes: None,
                    rank: None,
                },
            ]
        };
        let room = RoomView {
            room_id: "r1".into(),
            name: "스터디".into(),
            role: RoomRole::Owner,
            capacity: 8,
            members: vec![me, hidden],
            today_ranking: ranking(30),
            week_ranking: ranking(120),
            invites: vec![InviteView {
                invite_id: "i1".into(),
                expires_at_ms: 86_400_000,
                max_uses: 10,
                uses: 1,
                revoked: false,
                expired: false,
            }],
            schedule_updated_at_ms: Some(5),
            schedule: vec![TogetherItem {
                key: "1794013200000".into(),
                name: "수학 집중".into(),
                subject: "수학".into(),
                sets: 2,
                start_at_utc_ms: 1_794_013_200_000,
                local_date: "2026-11-07".into(),
                local_time: "10:00".into(),
                already_imported: false,
                in_past: false,
            }],
            fetched_at_ms: 10,
        };
        assert_eq!(
            json(&FeedView {
                room: Some(room),
                degraded_code: None,
                last_ok_ms: Some(10),
                poll_interval_ms: 5_000
            }),
            f["feed"]
        );
        let settings = FriendSettings {
            share_level: true,
            share_status: true,
            share_week_time: false,
            share_attempts: false,
            distraction_publish: false,
            distraction_receive: false,
        };
        assert_eq!(json(&settings), f["friendSettings"]);
        assert_eq!(
            json(&ImportResult {
                key: "1794013200000".into(),
                outcome: ImportOutcome::Applied,
                plan_id: Some(3)
            }),
            f["importResult"]
        );
        assert_eq!(
            json(&DistractionNotice {
                nickname: "나".into(),
                at_ms: 5
            }),
            f["notice"]
        );
    }
}
