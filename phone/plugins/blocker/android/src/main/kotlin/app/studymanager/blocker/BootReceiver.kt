package app.studymanager.blocker

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * After a reboot: bring a saved lock back, and report a service that is off while a lock is saved
 * as a gap (the app delivers it to the desktop on its next start).
 */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent?) {
        if (intent?.action != Intent.ACTION_BOOT_COMPLETED) return
        LockController.restore(context)
        if (LockController.isLocked() && !ServiceState.isEnabled(context)) {
            BlockerEvents.emit(context, BlockerEvents.SERVICE_DISABLED)
        }
    }
}
