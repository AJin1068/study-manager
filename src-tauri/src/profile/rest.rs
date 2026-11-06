//! The real profile adapter: every call is one Supabase RPC through `SupabaseRest`.
//!
//! Answers are `{"code": ...}` objects (see `fixtures/profile/rpc-contract.json`). A code other
//! than `ok` becomes `ProfileError::Rejected`. Request bodies are the sealed structs of
//! `profile::payload`.

use std::sync::Arc;

use serde_json::Value;

use super::model::{GiftState, InboxGift, ProfileCode, ProfileError, WireProfile};
use super::payload::{
    EntryIdArgs, GiftIdArgs, GiftKeyArgs, GrassArgs, GrassEntry, MemberArgs, SendGiftArgs, SetProfileArgs, SignGuestbookArgs,
};
use super::port::{AcceptedGift, ProfileApi};
use crate::remote::payload::{EgressPayload, NoArgs};
use crate::remote::supabase::{RemoteError, SupabaseRest};

pub struct RestProfile {
    rest: Arc<SupabaseRest>,
}

impl RestProfile {
    pub fn new(rest: Arc<SupabaseRest>) -> Self {
        Self { rest }
    }

    fn call<P: EgressPayload>(&self, kind: &'static str, name: &str, args: &P) -> Result<Value, ProfileError> {
        let answer = self.rest.rpc(kind, name, args)?;
        match answer.get("code").and_then(Value::as_str).and_then(ProfileCode::parse) {
            Some(ProfileCode::Ok) => Ok(answer),
            Some(code) => Err(ProfileError::Rejected(code)),
            None => Err(ProfileError::Remote(RemoteError::Decode)),
        }
    }

    fn text(answer: &Value, field: &str) -> Result<String, ProfileError> {
        answer.get(field).and_then(Value::as_str).map(str::to_string).ok_or(ProfileError::Remote(RemoteError::Decode))
    }

    fn profile(answer: &Value) -> Result<WireProfile, ProfileError> {
        serde_json::from_value(answer.clone()).map_err(|_| ProfileError::Remote(RemoteError::Decode))
    }
}

impl ProfileApi for RestProfile {
    fn set_settings(&self, args: &SetProfileArgs) -> Result<String, ProfileError> {
        Self::text(&self.call("rpc_set_profile_settings", "set_profile_settings", args)?, "slug")
    }

    fn report_grass(&self, entries: &[GrassEntry]) -> Result<(), ProfileError> {
        // The server takes at most 31 days per call.
        for chunk in entries.chunks(31) {
            self.call("rpc_report_profile_grass", "report_profile_grass", &GrassArgs { entries: chunk.to_vec() })?;
        }
        Ok(())
    }

    fn my_profile(&self) -> Result<WireProfile, ProfileError> {
        Self::profile(&self.call("rpc_get_my_profile", "get_my_profile", &NoArgs {})?)
    }

    fn member_profile(&self, user: &str) -> Result<WireProfile, ProfileError> {
        Self::profile(&self.call("rpc_get_member_profile", "get_member_profile", &MemberArgs { user: user.to_string() })?)
    }

    fn sign_guestbook(&self, owner: &str, body: &str) -> Result<(), ProfileError> {
        let args = SignGuestbookArgs { owner: owner.to_string(), body: body.to_string() };
        self.call("rpc_sign_guestbook", "sign_guestbook", &args).map(|_| ())
    }

    fn delete_entry(&self, id: i64) -> Result<(), ProfileError> {
        self.call("rpc_delete_guestbook_entry", "delete_guestbook_entry", &EntryIdArgs { id }).map(|_| ())
    }

    fn report_entry(&self, id: i64) -> Result<(), ProfileError> {
        self.call("rpc_report_guestbook_entry", "report_guestbook_entry", &EntryIdArgs { id }).map(|_| ())
    }

    fn send_gift(&self, args: &SendGiftArgs) -> Result<(), ProfileError> {
        self.call("rpc_send_gift", "send_gift", args).map(|_| ())
    }

    fn gift_status(&self, idem_key: &str) -> Result<GiftState, ProfileError> {
        let answer = self.call("rpc_gift_status", "gift_status", &GiftKeyArgs { idem_key: idem_key.to_string() })?;
        GiftState::parse(&Self::text(&answer, "state")?).ok_or(ProfileError::Remote(RemoteError::Decode))
    }

    fn gift_inbox(&self) -> Result<Vec<InboxGift>, ProfileError> {
        let answer = self.call("rpc_list_gift_inbox", "list_gift_inbox", &NoArgs {})?;
        serde_json::from_value(answer.get("gifts").cloned().unwrap_or(Value::Null)).map_err(|_| ProfileError::Remote(RemoteError::Decode))
    }

    fn accept_gift(&self, gift_id: &str) -> Result<AcceptedGift, ProfileError> {
        let answer = self.call("rpc_accept_gift", "accept_gift", &GiftIdArgs { gift_id: gift_id.to_string() })?;
        Ok(AcceptedGift { item_id: Self::text(&answer, "item_id")?, sender_name: Self::text(&answer, "sender_name")? })
    }

    fn decline_gift(&self, gift_id: &str) -> Result<(), ProfileError> {
        self.call("rpc_decline_gift", "decline_gift", &GiftIdArgs { gift_id: gift_id.to_string() }).map(|_| ())
    }
}
