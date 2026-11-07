//! Profile and gift commands. Calls that reach the server block, so they run on the blocking pool
//! with the database lock released.

use std::sync::Arc;

use tauri::State;

use crate::error::{AppError, AppResult};
use crate::profile::gift::{ReceiveOutcome, SendOutcome};
use crate::profile::model::{ProfileView, Scope, Section};
use crate::profile::service::{Ack, GiftOverview, HomeView, ProfileService, PublishView};

type Service<'a> = State<'a, Arc<ProfileService>>;

async fn blocking<T: Send + 'static>(
    service: &Service<'_>,
    work: impl FnOnce(&ProfileService) -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    let service = Arc::clone(service);
    tauri::async_runtime::spawn_blocking(move || work(&service))
        .await
        .map_err(|_| AppError::Internal("the profile job did not finish".into()))?
}

#[tauri::command]
pub async fn profile_home(service: Service<'_>) -> AppResult<HomeView> {
    blocking(&service, |s| s.home()).await
}

#[tauri::command]
pub async fn profile_set_name(service: Service<'_>, name: String) -> AppResult<Ack> {
    blocking(&service, move |s| s.set_name(&name)).await
}

#[tauri::command]
pub async fn profile_set_scope(
    service: Service<'_>,
    section: Section,
    scope: Scope,
) -> AppResult<PublishView> {
    blocking(&service, move |s| s.set_scope(section, scope)).await
}

#[tauri::command]
pub async fn profile_set_wiki_titles(service: Service<'_>, titles: Vec<String>) -> AppResult<Ack> {
    blocking(&service, move |s| s.set_wiki_titles(&titles)).await
}

#[tauri::command]
pub async fn profile_set_web_base(service: Service<'_>, base: String) -> AppResult<Ack> {
    blocking(&service, move |s| s.set_web_base(&base)).await
}

#[tauri::command]
pub async fn profile_publish(service: Service<'_>) -> AppResult<PublishView> {
    blocking(&service, |s| s.publish()).await
}

#[tauri::command]
pub async fn profile_my_view(service: Service<'_>) -> AppResult<ProfileView> {
    blocking(&service, |s| s.my_view()).await
}

#[tauri::command]
pub async fn profile_member_view(service: Service<'_>, user_id: String) -> AppResult<ProfileView> {
    blocking(&service, move |s| s.member_view(&user_id)).await
}

#[tauri::command]
pub async fn profile_sign_guestbook(
    service: Service<'_>,
    owner_id: String,
    body: String,
) -> AppResult<Ack> {
    blocking(&service, move |s| s.sign_guestbook(&owner_id, &body)).await
}

#[tauri::command]
pub async fn profile_delete_entry(service: Service<'_>, id: i64) -> AppResult<Ack> {
    blocking(&service, move |s| s.delete_entry(id)).await
}

#[tauri::command]
pub async fn profile_report_entry(service: Service<'_>, id: i64) -> AppResult<Ack> {
    blocking(&service, move |s| s.report_entry(id)).await
}

#[tauri::command]
pub async fn gift_send(
    service: Service<'_>,
    recipient_id: String,
    recipient_name: String,
    item_id: String,
    message: String,
) -> AppResult<SendOutcome> {
    blocking(&service, move |s| {
        s.send_gift(&recipient_id, &recipient_name, &item_id, &message)
    })
    .await
}

#[tauri::command]
pub async fn gift_overview(service: Service<'_>) -> AppResult<GiftOverview> {
    blocking(&service, |s| s.gift_overview()).await
}

#[tauri::command]
pub async fn gift_accept(service: Service<'_>, gift_id: String) -> AppResult<ReceiveOutcome> {
    blocking(&service, move |s| s.accept_gift(&gift_id)).await
}

#[tauri::command]
pub async fn gift_decline(service: Service<'_>, gift_id: String) -> AppResult<Ack> {
    blocking(&service, move |s| s.decline_gift(&gift_id)).await
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use crate::profile::gift::{ReceiveOutcome, SendOutcome};
    use crate::profile::model::*;
    use crate::profile::service::*;

    fn fixture() -> Value {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../src/api/fixtures/profile-contract.json"
        );
        serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json")
    }

    fn json<T: serde::Serialize>(value: &T) -> Value {
        serde_json::to_value(value).expect("serialize")
    }

    #[test]
    fn profile_wire_shapes_match_the_shared_fixture() {
        let f = fixture();
        let scopes: Vec<ScopeRow> = [
            (Section::Room, Scope::Friends),
            (Section::Grass, Scope::Public),
            (Section::Level, Scope::Public),
            (Section::Badges, Scope::Friends),
            (Section::Guestbook, Scope::Friends),
            (Section::Visitors, Scope::Private),
            (Section::Wiki, Scope::Private),
            (Section::Subject, Scope::Private),
            (Section::Gifts, Scope::Friends),
        ]
        .into_iter()
        .map(|(section, scope)| ScopeRow { section, scope })
        .collect();
        let home = HomeView {
            name: "민수".into(),
            slug: Some("0123456789abcdef".into()),
            link: Some("https://me.example.test/study/#/u/0123456789abcdef".into()),
            web_base_url: Some("https://me.example.test/study".into()),
            scopes,
            wiki_titles: vec!["미적분".into()],
            gift_daily_limit: 5,
            gift_message_max: 60,
        };
        assert_eq!(json(&home), f["home"]);
        assert_eq!(
            json(&Ack {
                code: ProfileCode::Ok
            }),
            f["ack"]
        );
        assert_eq!(
            json(&PublishView {
                code: ProfileCode::Ok,
                slug: Some("0123456789abcdef".into()),
                link: None
            }),
            f["publish"]
        );

        let view = ProfileView {
            code: ProfileCode::Ok,
            name: Some("민수".into()),
            slug: None,
            visible: vec![
                Section::Room,
                Section::Grass,
                Section::Level,
                Section::Badges,
                Section::Guestbook,
                Section::Visitors,
                Section::Gifts,
            ],
            room: Some(RoomLook {
                species: Some("fox".into()),
                hat: Some("hat_beanie".into()),
                top: None,
                accessory: None,
                hand: None,
                furniture: vec!["furn_lamp".into()],
            }),
            grass: Some(vec![GrassDay {
                day: "2026-11-05".into(),
                study_min: 90,
            }]),
            level: Some(7),
            badges: Some(vec!["streak_7".into()]),
            guestbook: Some(vec![GuestEntry {
                id: 3,
                author_name: "친구1".into(),
                body: "응원해요".into(),
                at_ms: 1000,
                mine: false,
                reported: false,
            }]),
            visitors: Some(Visitors { total: 4, today: 1 }),
            wiki: None,
            subject: None,
            gifts: Some(vec![GiftedItem {
                item_id: "acc_bowtie".into(),
                sender_name: "친구1".into(),
                message: "고마워".into(),
                at_ms: Some(2000),
            }]),
        };
        assert_eq!(json(&view), f["view"]);
        assert_eq!(
            json(&ProfileView::empty(ProfileCode::NotMember)),
            f["hiddenView"]
        );

        let id = "00000000-0000-4000-8000-00000000000a";
        assert_eq!(
            json(&SendOutcome {
                code: ProfileCode::Ok,
                gift_id: Some(id.into()),
                state: Some("pending".into()),
                balance: 60
            }),
            f["sendOutcome"]
        );
        assert_eq!(
            json(&SendOutcome {
                code: ProfileCode::NotGiftable,
                gift_id: None,
                state: None,
                balance: 100
            }),
            f["refusedSend"]
        );
        assert_eq!(
            json(&ReceiveOutcome {
                code: ProfileCode::Ok,
                item_id: Some("hat_beanie".into()),
                already_owned: false
            }),
            f["receiveOutcome"]
        );
        let overview = GiftOverview {
            outbox: vec![OutboxView {
                gift_id: id.into(),
                recipient_name: "친구".into(),
                item_id: "hat_beanie".into(),
                message: "고마워".into(),
                price: 40,
                state: "pending".into(),
                created_at_ms: 5,
            }],
            inbox: vec![InboxGift {
                gift_id: "00000000-0000-4000-8000-00000000000b".into(),
                item_id: "furn_lamp".into(),
                sender_name: "친구1".into(),
                message: "집들이".into(),
                status: "pending".into(),
                at_ms: 6,
            }],
            online: true,
        };
        assert_eq!(json(&overview), f["overview"]);
    }
}
