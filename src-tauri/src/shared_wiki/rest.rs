//! The real shared-wiki adapter: every call is one Supabase RPC through `SupabaseRest`. Answers are
//! `{"code": ...}` objects; a code other than `ok` becomes `SharedError::Rejected`.

use std::sync::Arc;

use serde_json::Value;

use super::model::{Published, SharedCode, SharedError, WirePage};
use super::payload::{PublishSharedPageArgs, SaveMergedPageArgs, WithdrawSharedPageArgs};
use super::port::SharedWikiPort;
use crate::remote::payload::{EgressPayload, RoomIdArgs};
use crate::remote::supabase::{RemoteError, SupabaseRest};

pub struct RestSharedWiki {
    rest: Arc<SupabaseRest>,
}

impl RestSharedWiki {
    pub fn new(rest: Arc<SupabaseRest>) -> Self {
        Self { rest }
    }

    fn call<P: EgressPayload>(
        &self,
        kind: &'static str,
        name: &str,
        args: &P,
    ) -> Result<Value, SharedError> {
        let answer = self.rest.rpc(kind, name, args)?;
        match answer
            .get("code")
            .and_then(Value::as_str)
            .and_then(SharedCode::parse)
        {
            Some(SharedCode::Ok) => Ok(answer),
            Some(code) => Err(SharedError::Rejected(code)),
            None => Err(SharedError::Remote(RemoteError::Decode)),
        }
    }

    fn text(answer: &Value, field: &str) -> Result<String, SharedError> {
        answer
            .get(field)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or(SharedError::Remote(RemoteError::Decode))
    }
}

impl SharedWikiPort for RestSharedWiki {
    fn publish(
        &self,
        room_id: &str,
        page_name: &str,
        body: &str,
        base_revision: Option<&str>,
        allow_llm_merge: bool,
    ) -> Result<Published, SharedError> {
        let args = PublishSharedPageArgs {
            room_id: room_id.to_string(),
            page_name: page_name.to_string(),
            body: body.to_string(),
            base_revision: base_revision.map(str::to_string),
            allow_llm_merge,
        };
        let answer = self.call("rpc_publish_shared_page", "publish_shared_page", &args)?;
        let branched = answer
            .get("branched")
            .and_then(Value::as_bool)
            .ok_or(SharedError::Remote(RemoteError::Decode))?;
        Ok(Published {
            revision_id: Self::text(&answer, "revision_id")?,
            branched,
        })
    }

    fn withdraw(&self, room_id: &str, page_name: &str) -> Result<usize, SharedError> {
        let args = WithdrawSharedPageArgs {
            room_id: room_id.to_string(),
            page_name: page_name.to_string(),
        };
        let answer = self.call("rpc_withdraw_shared_page", "withdraw_shared_page", &args)?;
        let count = answer
            .get("withdrawn")
            .and_then(Value::as_u64)
            .ok_or(SharedError::Remote(RemoteError::Decode))?;
        usize::try_from(count).map_err(|_| SharedError::Remote(RemoteError::Decode))
    }

    fn list(&self, room_id: &str) -> Result<Vec<WirePage>, SharedError> {
        let answer = self.call(
            "rpc_list_shared_pages",
            "list_shared_pages",
            &RoomIdArgs {
                room_id: room_id.to_string(),
            },
        )?;
        serde_json::from_value(answer.get("pages").cloned().unwrap_or(Value::Null))
            .map_err(|_| SharedError::Remote(RemoteError::Decode))
    }

    fn save_merged(
        &self,
        room_id: &str,
        page_name: &str,
        body: &str,
        source_revisions: &[String],
    ) -> Result<String, SharedError> {
        let args = SaveMergedPageArgs {
            room_id: room_id.to_string(),
            page_name: page_name.to_string(),
            body: body.to_string(),
            source_revisions: source_revisions.to_vec(),
        };
        Self::text(
            &self.call("rpc_save_merged_page", "save_merged_page", &args)?,
            "revision_id",
        )
    }
}
