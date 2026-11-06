package app.studymanager.blocker

import android.app.Activity
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class AllowArgs {
    lateinit var roles: Array<String>
    lateinit var packages: Array<String>
}

@InvokeArg
class EngageArgs {
    lateinit var run_id: String
    var seq: Long = 0
    var remaining_ms: Long = 0
    lateinit var allow: AllowArgs
}

/**
 * Tauri plugin `blocker`. Commands and replies are exactly those of
 * `fixtures/phone/blocker-bridge.json`; a refusal is a normal reply `{ ok: false, code }`, never a
 * rejected promise, so the Rust adapter has one shape to parse. Not built or run here (V5-4, V5-5).
 */
@TauriPlugin
class BlockerPlugin(private val activity: Activity) : Plugin(activity) {

    override fun load(webView: android.webkit.WebView) {
        super.load(webView)
        LockController.restore(activity)
        BlockerEvents.sink = { event -> trigger(BlockerEvents.CHANNEL, JSObject().put("event", event)) }
        BlockerEvents.sink?.let { sink -> LockStore.drainPending(activity).forEach(sink) }
    }

    @Command
    fun engage(invoke: Invoke) {
        val args = invoke.parseArgs(EngageArgs::class.java)
        val parsed = AllowList.parse(args.allow.roles.toList(), args.allow.packages.toList())
        if (parsed is AllowList.Result.Refused || args.remaining_ms < 1 || args.seq < 1 || args.run_id.isEmpty()) {
            invoke.resolve(refusal("invalid_request"))
            return
        }
        if (!ServiceState.isEnabled(activity)) {
            invoke.resolve(refusal("service_disabled"))
            return
        }
        val list = (parsed as AllowList.Result.Ok).list
        // A lower sequence number than the lock in force is a replayed old request.
        if (!LockController.engage(activity, args.run_id, args.seq, args.remaining_ms, list)) {
            invoke.resolve(refusal("invalid_request"))
            return
        }
        invoke.resolve(JSObject().put("ok", true))
    }

    @Command
    fun release(invoke: Invoke) {
        LockController.release(activity)
        invoke.resolve(JSObject().put("ok", true))
    }

    @Command
    fun status(invoke: Invoke) {
        val kind = when {
            !ServiceState.isEnabled(activity) -> "service_disabled"
            LockController.isLocked() -> "active"
            else -> "inactive"
        }
        invoke.resolve(JSObject().put("kind", kind))
    }

    private fun refusal(code: String) = JSObject().put("ok", false).put("code", code)
}
