package app.studymanager.blocker

import android.content.ComponentName
import android.content.Context
import android.provider.Settings

internal object ServiceState {
    fun isEnabled(context: Context): Boolean {
        val enabled = Settings.Secure.getString(context.contentResolver, Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES) ?: return false
        val mine = ComponentName(context, BlockerAccessibilityService::class.java).flattenToString()
        return enabled.split(':').any { it.equals(mine, ignoreCase = true) }
    }
}
