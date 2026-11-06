//! Records every outbound request in `remote_events` (kind, outcome, status, bytes sent; no
//! content). The sink is called after the network call returns, so the database lock is never
//! held across I/O.

use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use super::http::{HttpRequest, HttpResponse, HttpTransport, TransportError};
use super::Clock;
use crate::store::remote::{self as events, NewRemoteEvent};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressRecord {
    pub at_ms: i64,
    pub port: &'static str,
    pub kind: &'static str,
    pub outcome: &'static str,
    pub http_status: Option<u16>,
    pub bytes: i64,
    pub detail_code: Option<&'static str>,
}

pub trait EventSink: Send + Sync {
    fn record(&self, record: EgressRecord);
}

/// Writes records to the app database. A failed write is logged and dropped: auditing must not
/// turn into an outage of the feature being audited.
pub struct DbSink {
    db: Arc<Mutex<Connection>>,
}

impl DbSink {
    pub fn new(db: Arc<Mutex<Connection>>) -> Self {
        Self { db }
    }
}

impl EventSink for DbSink {
    fn record(&self, record: EgressRecord) {
        let conn = match crate::commands::lock_db(&self.db) {
            Ok(conn) => conn,
            Err(err) => {
                tracing::error!(event = "remote_event_dropped", error = %err);
                return;
            }
        };
        let row = NewRemoteEvent {
            at_ms: record.at_ms,
            port: record.port,
            kind: record.kind,
            outcome: record.outcome,
            http_status: record.http_status,
            bytes: record.bytes,
            detail_code: record.detail_code,
        };
        if let Err(err) = events::insert_event(&conn, &row) {
            tracing::error!(event = "remote_event_dropped", error = %err);
        }
    }
}

/// Decorates a transport so each request leaves an audit row.
pub struct AuditedTransport {
    inner: Arc<dyn HttpTransport>,
    sink: Arc<dyn EventSink>,
    clock: Arc<dyn Clock>,
}

impl AuditedTransport {
    pub fn new(inner: Arc<dyn HttpTransport>, sink: Arc<dyn EventSink>, clock: Arc<dyn Clock>) -> Self {
        Self { inner, sink, clock }
    }
}

fn classify(result: &Result<HttpResponse, TransportError>) -> (&'static str, Option<u16>, Option<&'static str>) {
    match result {
        Ok(response) if response.is_success() => ("ok", Some(response.status), None),
        Ok(response) if response.status == 429 => ("rate_limited", Some(response.status), None),
        Ok(response) => ("http_error", Some(response.status), None),
        Err(TransportError::HostDenied) => ("host_denied", None, Some("host_denied")),
        Err(err) => ("transport_error", None, Some(err.code())),
    }
}

impl HttpTransport for AuditedTransport {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        let tag = request.tag;
        let sent = request.body.as_ref().map_or(0, |b| i64::try_from(b.len()).unwrap_or(i64::MAX));
        let result = self.inner.send(request);
        let (outcome, http_status, detail_code) = classify(&result);
        match self.clock.now_ms() {
            Ok(at_ms) => self.sink.record(EgressRecord {
                at_ms,
                port: tag.port,
                kind: tag.kind,
                outcome,
                http_status,
                bytes: sent,
                detail_code,
            }),
            Err(err) => tracing::error!(event = "remote_event_dropped", error = %err),
        }
        result
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::remote::http::{HostPolicy, Method, RequestTag, ScriptedTransport};
    use crate::remote::payload::{Body, RefreshRequest};
    use crate::remote::ManualClock;

    #[derive(Default)]
    pub(crate) struct VecSink(pub Mutex<Vec<EgressRecord>>);

    impl EventSink for VecSink {
        fn record(&self, record: EgressRecord) {
            self.0.lock().expect("sink").push(record);
        }
    }

    #[test]
    fn every_outcome_is_recorded_without_content() {
        let scripted = Arc::new(ScriptedTransport::new(
            HostPolicy::LoopbackOnly,
            vec![
                Ok(HttpResponse::ok_json(200, "{}")),
                Ok(HttpResponse::ok_json(429, "")),
                Ok(HttpResponse::ok_json(500, "")),
                Err(TransportError::Timeout),
            ],
        ));
        let sink = Arc::new(VecSink::default());
        let audited = AuditedTransport::new(scripted, sink.clone(), Arc::new(ManualClock::new(77)));
        let tag = RequestTag { port: "supabase", kind: "auth_refresh" };
        let body = Body::from_payload(&RefreshRequest { refresh_token: "REFRESHSECRET".into() }).expect("body");
        let first = HttpRequest::new(tag, Method::Post, "http://127.0.0.1:1/x".into()).with_body(body);
        assert!(audited.send(first).is_ok());
        for _ in 0..3 {
            let _ = audited.send(HttpRequest::new(tag, Method::Get, "http://127.0.0.1:1/x".into()));
        }
        let denied = audited.send(HttpRequest::new(tag, Method::Get, "https://example.com/x".into()));
        assert_eq!(denied, Err(TransportError::HostDenied));

        let records = sink.0.lock().expect("records").clone();
        let outcomes: Vec<&str> = records.iter().map(|r| r.outcome).collect();
        assert_eq!(outcomes, ["ok", "rate_limited", "http_error", "transport_error", "host_denied"]);
        assert!(records[0].bytes > 0 && records[0].at_ms == 77);
        let dump = format!("{records:?}");
        assert!(!dump.contains("REFRESHSECRET"));
    }

    #[test]
    fn db_sink_writes_rows_that_hold_no_content() {
        let conn = crate::db::open_in_memory().expect("db");
        let db = Arc::new(Mutex::new(conn));
        let sink = DbSink::new(Arc::clone(&db));
        sink.record(EgressRecord {
            at_ms: 5,
            port: "supabase",
            kind: "rpc_heartbeat",
            outcome: "ok",
            http_status: Some(200),
            bytes: 42,
            detail_code: None,
        });
        let conn = db.lock().expect("lock");
        let rows = events::list_events(&conn, 10).expect("rows");
        assert_eq!((rows[0].kind.as_str(), rows[0].bytes), ("rpc_heartbeat", 42));
    }
}
