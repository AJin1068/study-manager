package app.studymanager.blocker

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.Telephony
import android.telecom.TelecomManager
import android.view.inputmethod.InputMethodManager

/**
 * Turns an [AllowList] into the package names that may be in front while locked.
 *
 * Always allowed, whatever the list says: this app, the system UI (notification shade, volume), the
 * keyboards, the home launcher (to reach an allowed app) and everything the emergency call needs
 * (the default dialer and the telephony/in-call surfaces). The settings app is never allowed.
 */
internal object AllowResolver {
    private val EMERGENCY_SURFACES = setOf(
        "com.android.phone",
        "com.android.server.telecom",
        "com.android.emergency",
        "com.android.incallui",
        "com.samsung.android.incallui",
    )

    fun allowedPackages(context: Context, list: AllowList): Set<String> {
        val packages = mutableSetOf(context.packageName, "com.android.systemui")
        packages += EMERGENCY_SURFACES
        context.getSystemService(TelecomManager::class.java)?.defaultDialerPackage?.let { packages += it }
        if (list.dialer) {
            val dial = Intent(Intent.ACTION_DIAL)
            context.packageManager.queryIntentActivities(dial, 0).forEach { packages += it.activityInfo.packageName }
        }
        if (list.messages) {
            Telephony.Sms.getDefaultSmsPackage(context)?.let { packages += it }
            val sms = Intent(Intent.ACTION_SENDTO, Uri.parse("smsto:"))
            context.packageManager.queryIntentActivities(sms, 0).forEach { packages += it.activityInfo.packageName }
        }
        val home = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_HOME)
        context.packageManager.queryIntentActivities(home, 0).forEach { packages += it.activityInfo.packageName }
        context.getSystemService(InputMethodManager::class.java)?.enabledInputMethodList?.forEach { packages += it.packageName }
        packages += list.packages
        return packages
    }
}
