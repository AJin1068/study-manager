package app.studymanager.blocker

import android.content.Context

/**
 * Plugin -> app events, `{ "event": <name> }` on the plugin event channel [CHANNEL]
 * (`fixtures/phone/blocker-bridge.json`, section `events`). An event raised while the app is not
 * listening is kept and delivered when the plugin loads, so a service switched off in the
 * background still reaches the desktop as a gap.
 */
object BlockerEvents {
    const val CHANNEL = "blocker"
    const val SERVICE_DISABLED = "service_disabled"
    const val SERVICE_ENABLED = "service_enabled"
    const val LOCK_ENDED = "lock_ended"

    @Volatile var sink: ((String) -> Unit)? = null

    fun emit(context: Context?, event: String) {
        val live = sink
        if (live != null) {
            live(event)
        } else if (context != null) {
            LockStore.addPending(context, event)
        }
    }
}
