//! The profile service: what the Tauri commands call. Network calls always run with the database
//! lock released; a server that is unreachable never changes what is stored locally except the
//! gift outbox, which is built to survive it.

use std::sync::{Arc, Mutex};

use chrono::Local;
use rusqlite::{Connection, TransactionBehavior};
use serde::Serialize;

use super::gift::{self, ReceiveOutcome, SendOutcome};
use super::link;
use super::model::{InboxGift, ProfileCode, ProfileError, ProfileView, Scope, Section};
use super::port::ProfileApi;
use super::publish::{self, MAX_WIKI_TITLES};
use super::rest::RestProfile;
use crate::commands::lock_db;
use crate::error::{AppError, AppResult};
use crate::remote::runtime::RemoteRuntime;
use crate::remote::supabase::RemoteError;
use crate::remote::Clock;
use crate::settings::{Outcome, Reason};
use crate::store::{self, profile as repo};

pub const NAME_MAX_CHARS: usize = 20;
pub const TITLE_MAX_CHARS: usize = 60;
pub const GUESTBOOK_MAX_CHARS: usize = 200;

/// Where the server port comes from: `None` when signed out or not configured.
pub trait ApiSource: Send + Sync {
    fn api(&self) -> Option<Arc<dyn ProfileApi>>;
}

struct AppApis {
    db: Arc<Mutex<Connection>>,
    runtime: Arc<RemoteRuntime>,
}

impl ApiSource for AppApis {
    fn api(&self) -> Option<Arc<dyn ProfileApi>> {
        let handle = {
            let conn = lock_db(&self.db).ok()?;
            self.runtime.supabase(&conn).ok()?
        };
        if !handle.auth.status().signed_in {
            return None;
        }
        Some(Arc::new(RestProfile::new(handle.rest)))
    }
}

// ---- views ----

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScopeRow {
    pub section: Section,
    pub scope: Scope,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OutboxView {
    pub gift_id: String,
    pub recipient_name: String,
    pub item_id: String,
    pub message: String,
    pub price: i64,
    /// `pending`, `sent` or `refunded`.
    pub state: String,
    pub created_at_ms: i64,
}

/// Everything the profile screen needs that lives on this PC. Reading it never touches the network.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HomeView {
    pub name: String,
    pub slug: Option<String>,
    /// The shareable web link; `None` until a web address and a published profile both exist.
    pub link: Option<String>,
    pub web_base_url: Option<String>,
    pub scopes: Vec<ScopeRow>,
    pub wiki_titles: Vec<String>,
    pub gift_daily_limit: i64,
    pub gift_message_max: usize,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublishView {
    pub code: ProfileCode,
    pub slug: Option<String>,
    pub link: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Ack {
    pub code: ProfileCode,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GiftOverview {
    pub outbox: Vec<OutboxView>,
    pub inbox: Vec<InboxGift>,
    /// Whether the server could be reached for this refresh.
    pub online: bool,
}

pub struct ProfileService {
    db: Arc<Mutex<Connection>>,
    clock: Arc<dyn Clock>,
    apis: Arc<dyn ApiSource>,
}

fn rejected_or_err<T>(result: Result<T, ProfileError>) -> AppResult<Result<T, ProfileCode>> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(ProfileError::Rejected(code)) => Ok(Err(code)),
        Err(other) => Err(other.into()),
    }
}

fn ack(result: Result<(), ProfileError>) -> AppResult<Ack> {
    Ok(Ack { code: rejected_or_err(result)?.err().unwrap_or(ProfileCode::Ok) })
}

impl ProfileService {
    pub fn new(db: Arc<Mutex<Connection>>, runtime: Arc<RemoteRuntime>, clock: Arc<dyn Clock>) -> Self {
        let apis = Arc::new(AppApis { db: Arc::clone(&db), runtime });
        Self::with_parts(db, clock, apis)
    }

    pub(crate) fn with_parts(db: Arc<Mutex<Connection>>, clock: Arc<dyn Clock>, apis: Arc<dyn ApiSource>) -> Self {
        Self { db, clock, apis }
    }

    fn api(&self) -> Result<Arc<dyn ProfileApi>, ProfileError> {
        self.apis.api().ok_or(ProfileError::Remote(RemoteError::NotSignedIn))
    }

    // ---- local settings ----

    pub fn home(&self) -> AppResult<HomeView> {
        let conn = lock_db(&self.db)?;
        let local = repo::local(&conn)?;
        let visibility = repo::visibility(&conn)?;
        Ok(HomeView {
            link: link::profile_link(local.web_base_url.as_deref(), local.slug.as_deref()),
            name: local.display_name,
            slug: local.slug,
            web_base_url: local.web_base_url,
            scopes: Section::ALL.into_iter().map(|section| ScopeRow { section, scope: visibility.get(&section).copied().unwrap_or(Scope::Private) }).collect(),
            wiki_titles: repo::wiki_titles(&conn)?,
            gift_daily_limit: gift::DAILY_LIMIT,
            gift_message_max: gift::MESSAGE_MAX_CHARS,
        })
    }

    /// Saves the profile name. An empty or too long name is refused (`invalid_input`).
    pub fn set_name(&self, name: &str) -> AppResult<Ack> {
        let name = name.trim();
        if name.chars().count() > NAME_MAX_CHARS || name.chars().any(char::is_control) {
            return Ok(Ack { code: ProfileCode::InvalidInput });
        }
        repo::save_name(&*lock_db(&self.db)?, name, self.clock.now_ms()?)?;
        Ok(Ack { code: ProfileCode::Ok })
    }

    /// Saves one section's scope, then publishes so a narrowed scope takes effect on the server
    /// at once (the server drops what the new scope no longer allows). The local save is kept
    /// even if the server cannot be reached; the next publish applies it.
    ///
    /// Like every privacy setting it is locked during a study run in the loosening direction
    /// (`run_locked`, recorded as `rejected_locked`); narrowing is allowed and recorded as
    /// `applied_privacy_tighten`. Every change is written to `setting_history`.
    pub fn set_scope(&self, section: Section, scope: Scope) -> AppResult<PublishView> {
        let at_ms = self.clock.now_ms()?;
        {
            let mut conn = lock_db(&self.db)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let before = repo::visibility(&tx)?.get(&section).copied().unwrap_or(Scope::Private);
            let key = format!("profile.scope.{}", section.as_str());
            let locked = crate::study::lock_held(&tx)?;
            if locked && scope.openness() > before.openness() {
                store::append_history(&tx, at_ms, &key, Some(before.as_str()), scope.as_str(), Reason::User, Outcome::RejectedLocked)?;
                tx.commit()?;
                return Ok(PublishView { code: ProfileCode::RunLocked, slug: None, link: None });
            }
            if scope != before {
                let outcome = if locked { Outcome::AppliedPrivacyTighten } else { Outcome::Applied };
                repo::set_visibility(&tx, section, scope, at_ms)?;
                store::append_history(&tx, at_ms, &key, Some(before.as_str()), scope.as_str(), Reason::User, outcome)?;
            }
            tx.commit()?;
        }
        self.publish()
    }

    /// Replaces the wiki page names shown on the profile (names only, never page text).
    pub fn set_wiki_titles(&self, titles: &[String]) -> AppResult<Ack> {
        let mut clean: Vec<String> = Vec::new();
        for title in titles {
            let title = title.trim();
            if title.is_empty() {
                continue;
            }
            if title.chars().count() > TITLE_MAX_CHARS || title.chars().any(char::is_control) {
                return Ok(Ack { code: ProfileCode::InvalidInput });
            }
            if !clean.iter().any(|t| t == title) {
                clean.push(title.to_string());
            }
        }
        if clean.len() > MAX_WIKI_TITLES {
            return Ok(Ack { code: ProfileCode::TooMany });
        }
        repo::replace_wiki_titles(&mut *lock_db(&self.db)?, &clean)?;
        Ok(Ack { code: ProfileCode::Ok })
    }

    /// Saves where the static web bundle is hosted; an empty text clears it.
    pub fn set_web_base(&self, raw: &str) -> AppResult<Ack> {
        let base = if raw.trim().is_empty() {
            None
        } else {
            match link::normalize_base(raw) {
                Some(base) => Some(base),
                None => return Ok(Ack { code: ProfileCode::InvalidInput }),
            }
        };
        repo::save_web_base(&*lock_db(&self.db)?, base.as_deref(), self.clock.now_ms()?)?;
        Ok(Ack { code: ProfileCode::Ok })
    }

    // ---- server ----

    /// Builds the payload from the local facts and the scopes and sends it. A failure to reach
    /// the server is reported as the error code of the link, not as a failed command.
    pub fn publish(&self) -> AppResult<PublishView> {
        let now = self.clock.now_ms()?;
        let facts = publish::collect(&*lock_db(&self.db)?, &Local, now)?;
        let sent = match self.api() {
            Ok(api) => publish::publish(api.as_ref(), &facts),
            Err(err) => Err(err),
        };
        match sent {
            Ok(slug) => {
                repo::save_slug(&*lock_db(&self.db)?, &slug, now)?;
                let base = repo::local(&*lock_db(&self.db)?)?.web_base_url;
                Ok(PublishView { code: ProfileCode::Ok, link: link::profile_link(base.as_deref(), Some(&slug)), slug: Some(slug) })
            }
            Err(ProfileError::Rejected(code)) => Ok(PublishView { code, slug: None, link: None }),
            Err(other) => Err(other.into()),
        }
    }

    pub fn my_view(&self) -> AppResult<ProfileView> {
        self.view_of(|api| api.my_profile())
    }

    pub fn member_view(&self, user: &str) -> AppResult<ProfileView> {
        let user = user.to_string();
        self.view_of(move |api| api.member_profile(&user))
    }

    fn view_of(&self, read: impl FnOnce(&dyn ProfileApi) -> Result<super::model::WireProfile, ProfileError>) -> AppResult<ProfileView> {
        let api = self.api()?;
        match read(api.as_ref()).and_then(ProfileView::from_wire) {
            Ok(view) => Ok(view),
            Err(ProfileError::Rejected(code)) => Ok(ProfileView::empty(code)),
            Err(other) => Err(other.into()),
        }
    }

    pub fn sign_guestbook(&self, owner: &str, body: &str) -> AppResult<Ack> {
        let body = body.trim();
        if body.is_empty() || body.chars().count() > GUESTBOOK_MAX_CHARS {
            return Ok(Ack { code: ProfileCode::InvalidInput });
        }
        ack(self.api()?.sign_guestbook(owner, body))
    }

    pub fn delete_entry(&self, id: i64) -> AppResult<Ack> {
        ack(self.api()?.delete_entry(id))
    }

    pub fn report_entry(&self, id: i64) -> AppResult<Ack> {
        ack(self.api()?.report_entry(id))
    }

    // ---- gifts ----

    pub fn send_gift(&self, recipient_user: &str, recipient_name: &str, item_id: &str, message: &str) -> AppResult<SendOutcome> {
        let api = self.api().map_err(AppError::from)?;
        let gift_id = uuid::Uuid::new_v4().to_string();
        gift::send(&self.db, api.as_ref(), &gift_id, recipient_user, recipient_name, item_id, message, self.clock.now_ms()?)
    }

    /// Refreshes the gift box. Offline, the local outbox is still returned.
    pub fn gift_overview(&self) -> AppResult<GiftOverview> {
        let now = self.clock.now_ms()?;
        let (inbox, online) = match self.apis.api() {
            Some(api) => {
                if let Err(err) = gift::reconcile(&self.db, api.as_ref(), now) {
                    tracing::warn!(event = "gift_reconcile_failed", error = %err);
                }
                match gift::sync_inbox(&self.db, api.as_ref(), now) {
                    Ok(inbox) => (inbox, true),
                    Err(err) => {
                        tracing::warn!(event = "gift_inbox_failed", error = %err);
                        (Vec::new(), false)
                    }
                }
            }
            None => (Vec::new(), false),
        };
        let outbox = repo::recent_outbox(&*lock_db(&self.db)?, 20)?
            .into_iter()
            .map(|r| OutboxView {
                gift_id: r.gift_id,
                recipient_name: r.recipient_name,
                item_id: r.item_id,
                message: r.message,
                price: r.price,
                state: r.state,
                created_at_ms: r.created_at_ms,
            })
            .collect();
        Ok(GiftOverview { outbox, inbox, online })
    }

    pub fn accept_gift(&self, gift_id: &str) -> AppResult<ReceiveOutcome> {
        let api = self.api()?;
        let now = self.clock.now_ms()?;
        let message = api.gift_inbox()?.into_iter().find(|g| g.gift_id == gift_id).map(|g| g.message).unwrap_or_default();
        gift::accept(&self.db, api.as_ref(), gift_id, &message, now)
    }

    pub fn decline_gift(&self, gift_id: &str) -> AppResult<Ack> {
        let api = self.api()?;
        Ok(Ack { code: gift::decline(&self.db, api.as_ref(), gift_id, self.clock.now_ms()?)? })
    }
}
