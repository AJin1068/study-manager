//! Gifts (S4.10): clothes or furniture bought with the sender's own points and given to a room
//! mate with a short message. Points never travel: the sender's points are spent, the recipient
//! receives an item they may accept or decline.
//!
//! Sending is local-first and idempotent:
//!   1. One Immediate transaction spends the points (`point_ledger` row `gift:<id>`) and writes the
//!      `gift_outbox` row `pending`. A crash after this point still has the whole story on disk.
//!   2. The RPC `send_gift` runs with the database lock released, keyed by the same `<id>`; the
//!      server processes a key once.
//!   3. Success marks the row `sent`. A CERTAIN refusal (the server's refusal code, a request that
//!      never left, a 4xx) refunds once (`gift_refund:<id>`). An unclear failure (timeout, lost
//!      connection, 5xx, unreadable answer) refunds NOTHING and leaves the row `pending`.
//!   4. `reconcile` asks the server with `gift_status(<id>)`: a gift the server has marks the row
//!      `sent` (no refund, the gift exists exactly once); a successful answer "none", read at least
//!      [`REFUND_GRACE_MS`] after the last attempt (long past any request timeout, so a delayed
//!      request cannot still arrive), refunds once.
//!
//! Refund and marking both go through `resolve_outbox`, which changes a row only while it is
//! `pending`, so a retry can never refund twice or refund a gift that was marked sent.

use std::sync::Mutex;

use rusqlite::{Connection, TransactionBehavior};
use serde::Serialize;

use super::model::{GiftState, InboxGift, ProfileCode, ProfileError};
use super::payload::SendGiftArgs;
use super::port::ProfileApi;
use crate::commands::lock_db;
use crate::error::AppResult;
use crate::points::rewards::{REASON_GIFT_REFUND, REASON_GIFT_SENT};
use crate::shop::items::{self, Item, ItemKind};
use crate::store;
use crate::store::profile::{self as repo, OutboxRow};

/// Gifts one PC sends in 24 hours (the server enforces the same number per account).
pub const DAILY_LIMIT: i64 = 5;
pub const MESSAGE_MAX_CHARS: usize = 60;
/// An unclear send is only declared lost this long after its last attempt.
pub const REFUND_GRACE_MS: i64 = 10 * 60_000;
const DAY_MS: i64 = 86_400_000;

/// Clothes and furniture sold in the shop. Achievement and starter items cannot be given, and
/// neither can an extra character.
pub fn giftable(item: &Item) -> bool {
    item.giftable() && item.kind != ItemKind::Character
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SendOutcome {
    pub code: ProfileCode,
    pub gift_id: Option<String>,
    /// `sent`, `pending` (unclear: nothing refunded, will be checked) or `refunded`.
    pub state: Option<String>,
    pub balance: i64,
}

fn refuse(code: ProfileCode, conn: &Connection) -> AppResult<SendOutcome> {
    Ok(SendOutcome {
        code,
        gift_id: None,
        state: None,
        balance: store::ledger_sums(conn)?.0,
    })
}

fn ledger_key(gift_id: &str) -> String {
    format!("gift:{gift_id}")
}

fn refund_key(gift_id: &str) -> String {
    format!("gift_refund:{gift_id}")
}

/// Spends the points and queues the gift, then tries to deliver it. `gift_id` is a fresh UUID.
#[allow(clippy::too_many_arguments)] // one parameter per gift field
pub fn send(
    db: &Mutex<Connection>,
    api: &dyn ProfileApi,
    gift_id: &str,
    recipient_user: &str,
    recipient_name: &str,
    item_id: &str,
    message: &str,
    now_ms: i64,
) -> AppResult<SendOutcome> {
    let message = message.trim();
    let name = recipient_name.trim();
    if message.chars().count() > MESSAGE_MAX_CHARS
        || recipient_user.is_empty()
        || !(1..=20).contains(&name.chars().count())
    {
        return refuse(ProfileCode::InvalidInput, &*lock_db(db)?);
    }
    let Some(item) = items::item(item_id)? else {
        return refuse(ProfileCode::UnknownItem, &*lock_db(db)?);
    };
    if !giftable(item) {
        return refuse(ProfileCode::NotGiftable, &*lock_db(db)?);
    }
    {
        let mut conn = lock_db(db)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if repo::sent_since(&tx, now_ms - DAY_MS)? >= DAILY_LIMIT {
            return refuse(ProfileCode::DailyLimit, &tx);
        }
        let (balance, _) = store::ledger_sums(&tx)?;
        if balance < item.price {
            return refuse(ProfileCode::InsufficientPoints, &tx);
        }
        if !store::insert_ledger_entry(
            &tx,
            now_ms,
            -item.price,
            REASON_GIFT_SENT,
            &ledger_key(gift_id),
        )? {
            return refuse(ProfileCode::KeyConflict, &tx);
        }
        repo::insert_outbox(
            &tx,
            &OutboxRow {
                gift_id: gift_id.to_string(),
                recipient_user: recipient_user.to_string(),
                recipient_name: name.to_string(),
                item_id: item.id.clone(),
                message: message.to_string(),
                price: item.price,
                state: "pending".into(),
                last_code: None,
                attempts: 0,
                created_at_ms: now_ms,
                last_attempt_ms: None,
            },
        )?;
        tx.commit()?;
    }
    let row = repo::outbox(&*lock_db(db)?, gift_id)?;
    match row {
        Some(row) => deliver(db, api, &row, now_ms),
        None => refuse(ProfileCode::NotFound, &*lock_db(db)?),
    }
}

fn outcome(db: &Mutex<Connection>, gift_id: &str, code: ProfileCode) -> AppResult<SendOutcome> {
    let conn = lock_db(db)?;
    let state = repo::outbox(&conn, gift_id)?.map(|r| r.state);
    Ok(SendOutcome {
        code,
        gift_id: Some(gift_id.to_string()),
        state,
        balance: store::ledger_sums(&conn)?.0,
    })
}

/// Sends the queued gift once and settles the row according to how the attempt ended.
fn deliver(
    db: &Mutex<Connection>,
    api: &dyn ProfileApi,
    row: &OutboxRow,
    now_ms: i64,
) -> AppResult<SendOutcome> {
    let args = SendGiftArgs {
        idem_key: row.gift_id.clone(),
        recipient: row.recipient_user.clone(),
        item_id: row.item_id.clone(),
        message: row.message.clone(),
    };
    match api.send_gift(&args) {
        Ok(()) => {
            resolve(db, row, "sent", now_ms)?;
            tracing::info!(event = "gift", result = "sent");
            outcome(db, &row.gift_id, ProfileCode::Ok)
        }
        Err(err) if err.is_certain_refusal() => {
            resolve(db, row, "refunded", now_ms)?;
            tracing::info!(event = "gift", result = "refused", code = err.code());
            let code = match err {
                ProfileError::Rejected(code) => code,
                ProfileError::Remote(_) => ProfileCode::NotSignedIn,
            };
            outcome(db, &row.gift_id, code)
        }
        Err(err) => {
            repo::note_attempt(&*lock_db(db)?, &row.gift_id, err.code(), now_ms)?;
            tracing::warn!(event = "gift", result = "unclear", code = err.code());
            outcome(db, &row.gift_id, ProfileCode::Ok)
        }
    }
}

/// Moves a pending row to `sent` or `refunded`; the refund row is written in the same transaction
/// and only by the call that actually changed the state.
fn resolve(db: &Mutex<Connection>, row: &OutboxRow, state: &str, now_ms: i64) -> AppResult<()> {
    let mut conn = lock_db(db)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if repo::resolve_outbox(&tx, &row.gift_id, state, now_ms)? && state == "refunded" {
        store::insert_ledger_entry(
            &tx,
            now_ms,
            row.price,
            REASON_GIFT_REFUND,
            &refund_key(&row.gift_id),
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Settles every pending gift by asking the server. Returns how many rows changed.
pub fn reconcile(db: &Mutex<Connection>, api: &dyn ProfileApi, now_ms: i64) -> AppResult<usize> {
    let pending = repo::pending_outbox(&*lock_db(db)?)?;
    let mut changed = 0;
    for row in pending {
        match api.gift_status(&row.gift_id) {
            Ok(GiftState::None) => {
                let since = row.last_attempt_ms.unwrap_or(row.created_at_ms);
                if now_ms - since >= REFUND_GRACE_MS {
                    resolve(db, &row, "refunded", now_ms)?;
                    changed += 1;
                }
            }
            Ok(_) => {
                resolve(db, &row, "sent", now_ms)?;
                changed += 1;
            }
            Err(err) => tracing::warn!(event = "gift_reconcile", code = err.code()),
        }
    }
    Ok(changed)
}

// ---- receiving ----

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveOutcome {
    pub code: ProfileCode,
    pub item_id: Option<String>,
    pub already_owned: bool,
}

/// Puts an accepted gift into the inventory once and remembers it.
fn apply_accepted(
    db: &Mutex<Connection>,
    gift_id: &str,
    item_id: &str,
    sender: &str,
    message: &str,
    now_ms: i64,
) -> AppResult<bool> {
    let conn = lock_db(db)?;
    let known = items::item(item_id)?.is_some_and(giftable);
    let added = known && store::shop::insert_inventory(&conn, item_id, "gift", now_ms, None)?;
    repo::record_received(&conn, gift_id, item_id, sender, message, "accepted", now_ms)?;
    Ok(!added)
}

pub fn accept(
    db: &Mutex<Connection>,
    api: &dyn ProfileApi,
    gift_id: &str,
    message: &str,
    now_ms: i64,
) -> AppResult<ReceiveOutcome> {
    match api.accept_gift(gift_id) {
        Ok(accepted) => {
            let already_owned = apply_accepted(
                db,
                gift_id,
                &accepted.item_id,
                &accepted.sender_name,
                message,
                now_ms,
            )?;
            Ok(ReceiveOutcome {
                code: ProfileCode::Ok,
                item_id: Some(accepted.item_id),
                already_owned,
            })
        }
        Err(ProfileError::Rejected(code)) => Ok(ReceiveOutcome {
            code,
            item_id: None,
            already_owned: false,
        }),
        Err(other) => Err(other.into()),
    }
}

pub fn decline(
    db: &Mutex<Connection>,
    api: &dyn ProfileApi,
    gift_id: &str,
    now_ms: i64,
) -> AppResult<ProfileCode> {
    match api.decline_gift(gift_id) {
        Ok(()) => {
            repo::record_received(&*lock_db(db)?, gift_id, "", "", "", "declined", now_ms)?;
            Ok(ProfileCode::Ok)
        }
        Err(ProfileError::Rejected(code)) => Ok(code),
        Err(other) => Err(other.into()),
    }
}

/// Reads the gift box. A gift the server shows as accepted but this PC has not applied (the
/// answer to an accept was lost) is applied now, so an accept is never half done.
pub fn sync_inbox(
    db: &Mutex<Connection>,
    api: &dyn ProfileApi,
    now_ms: i64,
) -> AppResult<Vec<InboxGift>> {
    let gifts = api.gift_inbox()?;
    let mut waiting = Vec::new();
    for gift in gifts {
        match gift.status.as_str() {
            "pending" => waiting.push(gift),
            "accepted" if repo::received_outcome(&*lock_db(db)?, &gift.gift_id)?.is_none() => {
                apply_accepted(
                    db,
                    &gift.gift_id,
                    &gift.item_id,
                    &gift.sender_name,
                    &gift.message,
                    now_ms,
                )?;
            }
            _ => {}
        }
    }
    Ok(waiting)
}
