package app.studymanager.blocker

import android.content.Context

/** The engaged lock, kept so a restart or a reboot can restore it. Nothing here names an app the user used. */
internal object LockStore {
    private const val FILE = "study_manager_blocker"

    class Saved(
        val runId: String,
        val seq: Long,
        val remainingAtEngageMs: Long,
        val deadlineElapsed: Long,
        val wallDeadlineMs: Long,
        val bootCount: Int,
        val allow: AllowList,
    )

    fun save(context: Context, runId: String, seq: Long, remainingMs: Long, deadlineElapsed: Long, bootCount: Int, allow: AllowList) {
        context.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit()
            .putString("run_id", runId)
            .putLong("seq", seq)
            .putLong("remaining_at_engage_ms", remainingMs)
            .putLong("deadline_elapsed", deadlineElapsed)
            .putLong("wall_deadline_ms", System.currentTimeMillis() + remainingMs)
            .putInt("boot_count", bootCount)
            .putBoolean("dialer", allow.dialer)
            .putBoolean("messages", allow.messages)
            .putStringSet("packages", allow.packages)
            .apply()
    }

    fun load(context: Context): Saved? {
        val prefs = context.getSharedPreferences(FILE, Context.MODE_PRIVATE)
        val runId = prefs.getString("run_id", null) ?: return null
        val roles = buildList {
            add(AllowList.ROLE_EMERGENCY_CALL)
            if (prefs.getBoolean("dialer", true)) add(AllowList.ROLE_DIALER)
            if (prefs.getBoolean("messages", true)) add(AllowList.ROLE_MESSAGES)
        }
        val allow = (AllowList.parse(roles, prefs.getStringSet("packages", emptySet())!!.toList()) as? AllowList.Result.Ok)?.list
            ?: return null
        return Saved(
            runId = runId,
            seq = prefs.getLong("seq", 0),
            remainingAtEngageMs = prefs.getLong("remaining_at_engage_ms", 0),
            deadlineElapsed = prefs.getLong("deadline_elapsed", 0),
            wallDeadlineMs = prefs.getLong("wall_deadline_ms", 0),
            bootCount = prefs.getInt("boot_count", -1),
            allow = allow,
        )
    }

    fun clear(context: Context) {
        context.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit()
            .remove("run_id").remove("seq").remove("remaining_at_engage_ms").remove("deadline_elapsed")
            .remove("wall_deadline_ms").remove("boot_count").remove("dialer").remove("messages").remove("packages")
            .apply()
    }

    /** Events that happened while no plugin was listening (WebView gone, boot). Drained on plugin load. */
    fun addPending(context: Context, event: String) {
        val prefs = context.getSharedPreferences(FILE, Context.MODE_PRIVATE)
        val pending = prefs.getStringSet("pending_events", emptySet())!!.toMutableSet()
        pending.add(event)
        prefs.edit().putStringSet("pending_events", pending).apply()
    }

    fun drainPending(context: Context): Set<String> {
        val prefs = context.getSharedPreferences(FILE, Context.MODE_PRIVATE)
        val pending = prefs.getStringSet("pending_events", emptySet())!!.toSet()
        prefs.edit().remove("pending_events").apply()
        return pending
    }
}
