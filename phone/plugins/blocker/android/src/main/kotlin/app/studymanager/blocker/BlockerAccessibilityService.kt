package app.studymanager.blocker

import android.accessibilityservice.AccessibilityService
import android.content.Intent
import android.view.accessibility.AccessibilityEvent

/**
 * Watches the package name of the window in front. While a lock is in force, a package outside the
 * allow list gets the full-screen [BlockActivity]. The settings app is never on the list, so it is
 * blocked like any other app: the user cannot switch this service off while the lock holds.
 *
 * Privacy: the package name is compared in memory and dropped. It is not stored, logged, counted
 * or sent anywhere.
 */
class BlockerAccessibilityService : AccessibilityService() {
    private var allowedFor: AllowList? = null
    private var allowed: Set<String> = emptySet()

    override fun onServiceConnected() {
        super.onServiceConnected()
        LockController.restore(this)
        BlockerEvents.emit(this, BlockerEvents.SERVICE_ENABLED)
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        if (event?.eventType != AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED) return
        val list = LockController.allowList() ?: return
        val pkg = event.packageName?.toString() ?: return
        if (allowedFor !== list) {
            allowed = AllowResolver.allowedPackages(this, list)
            allowedFor = list
        }
        if (pkg in allowed) return
        startActivity(
            Intent(this, BlockActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP),
        )
    }

    override fun onInterrupt() = Unit

    /** Called when the user switches the service off, or the system stops it. */
    override fun onUnbind(intent: Intent?): Boolean {
        if (LockController.isLocked()) BlockerEvents.emit(this, BlockerEvents.SERVICE_DISABLED)
        return super.onUnbind(intent)
    }
}
