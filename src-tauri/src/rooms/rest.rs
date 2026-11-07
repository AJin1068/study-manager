//! The real room adapter: every call is one Supabase RPC through `SupabaseRest`.
//!
//! Answers are `{"code": ...}` objects (see `fixtures/rooms/rpc-contract.json`). A code other than
//! `ok` becomes `RoomError::Rejected`. Request bodies are the sealed structs of `remote::payload`.

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::Value;

use super::model::{
    CreatedInvite, RoomCode, RoomError, RoomSummary, WireDistraction, WireRoom, WireSnapshot,
};
use super::port::{RoomFeed, RoomReporter, RoomService};
use crate::remote::payload::{
    CreateInviteArgs, CreateRoomArgs, DailyReportArgs, EgressPayload, FriendPrefsArgs,
    InventoryPublicArgs, JoinRoomArgs, KickMemberArgs, NoArgs, PollDistractionArgs,
    RevokeInviteArgs, RoomIdArgs, ScheduleItemRow, ShareScheduleArgs, StatusReportArgs,
};
use crate::remote::supabase::{RemoteError, SupabaseRest};

pub struct RestRooms {
    rest: Arc<SupabaseRest>,
}

impl RestRooms {
    pub fn new(rest: Arc<SupabaseRest>) -> Self {
        Self { rest }
    }

    /// Calls an RPC and returns its answer when the code is `ok`.
    fn call<P: EgressPayload>(
        &self,
        kind: &'static str,
        name: &str,
        args: &P,
    ) -> Result<Value, RoomError> {
        let answer = self.rest.rpc(kind, name, args)?;
        match answer
            .get("code")
            .and_then(Value::as_str)
            .and_then(RoomCode::parse)
        {
            Some(RoomCode::Ok) => Ok(answer),
            Some(code) => Err(RoomError::Rejected(code)),
            None => Err(RoomError::Remote(RemoteError::Decode)),
        }
    }

    fn decode<T: DeserializeOwned>(value: &Value) -> Result<T, RoomError> {
        serde_json::from_value(value.clone()).map_err(|_| RoomError::Remote(RemoteError::Decode))
    }

    fn text(answer: &Value, field: &str) -> Result<String, RoomError> {
        answer
            .get(field)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or(RoomError::Remote(RemoteError::Decode))
    }
}

impl RoomService for RestRooms {
    fn create_room(&self, name: &str, nickname: &str) -> Result<String, RoomError> {
        let args = CreateRoomArgs {
            name: name.to_string(),
            nickname: nickname.to_string(),
        };
        Self::text(
            &self.call("rpc_create_room", "create_room", &args)?,
            "room_id",
        )
    }

    fn create_invite(
        &self,
        room_id: &str,
        ttl_hours: i64,
        max_uses: i64,
    ) -> Result<CreatedInvite, RoomError> {
        let args = CreateInviteArgs {
            room_id: room_id.to_string(),
            ttl_hours,
            max_uses,
        };
        let answer = self.call("rpc_create_invite", "create_invite", &args)?;
        Ok(CreatedInvite {
            invite_id: Self::text(&answer, "invite_id")?,
            invite_code: Self::text(&answer, "invite_code")?,
            expires_at_ms: answer
                .get("expires_at_ms")
                .and_then(Value::as_i64)
                .ok_or(RoomError::Remote(RemoteError::Decode))?,
        })
    }

    fn revoke_invite(&self, room_id: &str, invite_id: &str) -> Result<(), RoomError> {
        let args = RevokeInviteArgs {
            room_id: room_id.to_string(),
            invite_id: invite_id.to_string(),
        };
        self.call("rpc_revoke_invite", "revoke_invite", &args)
            .map(|_| ())
    }

    fn join_room(&self, code: &str, nickname: &str) -> Result<String, RoomError> {
        let args = JoinRoomArgs {
            code: code.to_string(),
            nickname: nickname.to_string(),
        };
        Self::text(&self.call("rpc_join_room", "join_room", &args)?, "room_id")
    }

    fn leave_room(&self, room_id: &str) -> Result<(), RoomError> {
        self.call(
            "rpc_leave_room",
            "leave_room",
            &RoomIdArgs {
                room_id: room_id.to_string(),
            },
        )
        .map(|_| ())
    }

    fn kick_member(&self, room_id: &str, target_user: &str) -> Result<(), RoomError> {
        let args = KickMemberArgs {
            room_id: room_id.to_string(),
            target_user: target_user.to_string(),
        };
        self.call("rpc_kick_member", "kick_member", &args)
            .map(|_| ())
    }

    fn list_rooms(&self) -> Result<Vec<RoomSummary>, RoomError> {
        let answer = self.call("rpc_list_my_rooms", "list_my_rooms", &NoArgs {})?;
        let rooms: Vec<WireRoom> = Self::decode(answer.get("rooms").unwrap_or(&Value::Null))?;
        Ok(rooms.into_iter().map(RoomSummary::from).collect())
    }
}

impl RoomFeed for RestRooms {
    fn snapshot(&self, room_id: &str) -> Result<WireSnapshot, RoomError> {
        let answer = self.call(
            "rpc_room_snapshot",
            "room_snapshot",
            &RoomIdArgs {
                room_id: room_id.to_string(),
            },
        )?;
        Self::decode(&answer)
    }

    fn poll_distractions(&self, since_ms: i64) -> Result<Vec<WireDistraction>, RoomError> {
        match self.call(
            "rpc_poll_distractions",
            "poll_distraction_events",
            &PollDistractionArgs { since_ms },
        ) {
            Ok(answer) => Self::decode(answer.get("events").unwrap_or(&Value::Null)),
            Err(RoomError::Rejected(RoomCode::Disabled)) => Ok(Vec::new()),
            Err(err) => Err(err),
        }
    }
}

impl RoomReporter for RestRooms {
    fn set_prefs(&self, prefs: &FriendPrefsArgs) -> Result<(), RoomError> {
        self.call("rpc_set_friend_prefs", "set_friend_prefs", prefs)
            .map(|_| ())
    }

    fn report_status(&self, report: &StatusReportArgs) -> Result<(), RoomError> {
        self.call("rpc_report_status", "report_status", report)
            .map(|_| ())
    }

    fn report_daily(&self, report: &DailyReportArgs) -> Result<(), RoomError> {
        self.call("rpc_report_daily", "report_daily", report)
            .map(|_| ())
    }

    fn report_distraction(&self) -> Result<(), RoomError> {
        match self.call("rpc_report_distraction", "report_distraction", &NoArgs {}) {
            Ok(_) | Err(RoomError::Rejected(RoomCode::Throttled)) => Ok(()),
            Err(err) => Err(err),
        }
    }

    fn set_inventory_public(&self, args: &InventoryPublicArgs) -> Result<(), RoomError> {
        self.call("rpc_set_inventory_public", "set_inventory_public", args)
            .map(|_| ())
    }

    fn share_schedule(&self, room_id: &str, items: &[ScheduleItemRow]) -> Result<(), RoomError> {
        let args = ShareScheduleArgs {
            room_id: room_id.to_string(),
            items: items.to_vec(),
        };
        self.call("rpc_share_schedule", "share_schedule", &args)
            .map(|_| ())
    }
}
