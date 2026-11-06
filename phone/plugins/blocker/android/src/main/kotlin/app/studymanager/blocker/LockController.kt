package app.studymanager.blocker

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.Settings

/**
 * The lock in force on this device. One per process, shared by the plugin and the accessibility
 * service.
 *
 * The end of the lock is a deadline on the MONOTONIC clock ([SystemClock.elapsedRealtime]), so
 * changing the wall clock cannot shorten or extend it, and it ends here even if the app, the
 * WebView or the network is gone: no lock outlives its lease.
 *
 * After a reboot the monotonic clock restarts. [restore] then bounds the remaining time by both
 * what was left when the lock was engaged and the saved wall-clock deadline; the app fetches the
 * lease again and engages with the exact value as soon as it runs. Unverified on a device (V5-5).
 */
object LockController {
    private val main = Handler(Looper.getMainLooper())
    private val expire = Runnable { onDeadline() }

    @Volatile private var runId: String? = null
    @Volatile private var seq: Long = 0
    @Volatile private var deadlineElapsed: Long = 0
    @Volatile private var allow: AllowList? = null
    private var appContext: Context? = null

    fun isLocked(): Boolean {
        if (runId == null) return false
        if (SystemClock.elapsedRealtime() >= deadlineElapsed) {
            onDeadline()
            return false
        }
        return true
    }

    fun remainingMs(): Long = if (isLocked()) deadlineElapsed - SystemClock.elapsedRealtime() else 0

    fun allowList(): AllowList? = if (isLocked()) allow else null

    /** Starts the lock, or replaces it when the sequence number is not lower. */
    fun engage(context: Context, run: String, sequence: Long, remainingMs: Long, list: AllowList): Boolean {
        if (runId != null && sequence < seq) return false
        appContext = context.applicationContext
        runId = run
        seq = sequence
        allow = list
        deadlineElapsed = SystemClock.elapsedRealtime() + remainingMs
        LockStore.save(context, run, sequence, remainingMs, deadlineElapsed, bootCount(context), list)
        schedule(remainingMs)
        return true
    }

    fun release(context: Context) {
        main.removeCallbacks(expire)
        runId = null
        allow = null
        appContext = context.applicationContext
        LockStore.clear(context)
    }

    /** Loads a saved lock after a process restart or a reboot. */
    fun restore(context: Context) {
        if (runId != null) return
        val saved = LockStore.load(context) ?: return
        val remaining = if (saved.bootCount == bootCount(context)) {
            saved.deadlineElapsed - SystemClock.elapsedRealtime()
        } else {
            minOf(saved.remainingAtEngageMs, saved.wallDeadlineMs - System.currentTimeMillis())
        }
        if (remaining <= 0) {
            LockStore.clear(context)
            return
        }
        appContext = context.applicationContext
        runId = saved.runId
        seq = saved.seq
        allow = saved.allow
        deadlineElapsed = SystemClock.elapsedRealtime() + remaining
        schedule(remaining)
    }

    private fun schedule(delayMs: Long) {
        main.removeCallbacks(expire)
        main.postDelayed(expire, delayMs)
    }

    private fun onDeadline() {
        val context = appContext
        val wasLocked = runId != null
        main.removeCallbacks(expire)
        runId = null
        allow = null
        if (context != null) LockStore.clear(context)
        if (wasLocked) BlockerEvents.emit(context, BlockerEvents.LOCK_ENDED)
    }

    private fun bootCount(context: Context): Int =
        Settings.Global.getInt(context.contentResolver, Settings.Global.BOOT_COUNT, -1)
}
