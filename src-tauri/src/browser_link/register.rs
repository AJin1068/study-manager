//! Installing and removing the native-messaging host manifest, as a user action from settings.
//! Removing it is a way to switch site blocking off (Chrome can no longer start the host), so
//! removal follows the study lock like any other setting; installing only widens blocking and is
//! always allowed. Both leave a `setting_history` row.

use std::path::Path;

use rusqlite::{Connection, TransactionBehavior};

use super::ALLOWED_EXTENSION_IDS;
use crate::error::AppResult;
use crate::platform;
use crate::settings::{Outcome, Reason, UpdateOutcome};
use crate::store;

pub const HOST_HISTORY_KEY: &str = "browserHost";

pub fn set_registered(
    conn: &mut Connection,
    manifest_dir: &Path,
    host_path: &Path,
    register: bool,
    at_ms: i64,
) -> AppResult<UpdateOutcome> {
    let before = if platform::is_host_registered(manifest_dir) { "registered" } else { "unregistered" };
    let after = if register { "registered" } else { "unregistered" };
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if !register && crate::study::lock_held(&tx)? {
        store::append_history(&tx, at_ms, HOST_HISTORY_KEY, Some(before), after, Reason::User, Outcome::RejectedLocked)?;
        tx.commit()?;
        return Ok(UpdateOutcome::RejectedLocked { key: HOST_HISTORY_KEY.into() });
    }
    if register {
        platform::register_host(manifest_dir, host_path, ALLOWED_EXTENSION_IDS)?;
    } else {
        platform::unregister_host(manifest_dir)?;
    }
    store::append_history(&tx, at_ms, HOST_HISTORY_KEY, Some(before), after, Reason::User, Outcome::Applied)?;
    tx.commit()?;
    Ok(UpdateOutcome::Applied { key: HOST_HISTORY_KEY.into() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::settings::read_history;
    use crate::testutil::set_lock_for_test;

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("study-manager-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn register_then_unregister_follows_the_lock_for_removal_only() {
        let dir = scratch_dir("register");
        let host = Path::new("/tmp/study-manager-host");
        let mut conn = open_in_memory().expect("db");
        set_registered(&mut conn, &dir, host, true, 1).expect("register");
        assert!(platform::is_host_registered(&dir));
        let file = std::fs::read_dir(&dir).expect("dir").next().expect("one file").expect("entry").path();
        let manifest = std::fs::read_to_string(file).expect("manifest");
        assert!(manifest.contains(&format!("chrome-extension://{}/", ALLOWED_EXTENSION_IDS[0])));

        set_lock_for_test(&conn, true);
        let refused = set_registered(&mut conn, &dir, host, false, 2).expect("attempt");
        assert!(matches!(refused, UpdateOutcome::RejectedLocked { .. }));
        assert!(platform::is_host_registered(&dir), "removal must not happen under the lock");
        let latest = &read_history(&conn, 1).expect("history")[0];
        assert_eq!((latest.key.as_str(), latest.outcome.as_str(), latest.after.as_str()), (HOST_HISTORY_KEY, "rejected_locked", "unregistered"));
        // Installing while locked only strengthens blocking.
        assert!(matches!(set_registered(&mut conn, &dir, host, true, 3).expect("again"), UpdateOutcome::Applied { .. }));

        set_lock_for_test(&conn, false);
        assert!(matches!(set_registered(&mut conn, &dir, host, false, 4).expect("remove"), UpdateOutcome::Applied { .. }));
        assert!(!platform::is_host_registered(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
