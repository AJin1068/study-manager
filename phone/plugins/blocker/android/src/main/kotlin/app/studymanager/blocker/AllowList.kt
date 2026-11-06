package app.studymanager.blocker

/**
 * The allow list as the Rust side sends it (`fixtures/phone/blocker-bridge.json`, section
 * `allowList`). Pure Kotlin so the unit test runs on the JVM against the shared fixture.
 *
 * The emergency call is not optional: [allowsEmergencyCall] is a constant and [toWireRoles] always
 * starts with it, whatever the caller passed.
 */
class AllowList private constructor(
    val dialer: Boolean,
    val messages: Boolean,
    val packages: Set<String>,
) {
    val allowsEmergencyCall: Boolean get() = true

    fun toWireRoles(): List<String> = buildList {
        add(ROLE_EMERGENCY_CALL)
        if (dialer) add(ROLE_DIALER)
        if (messages) add(ROLE_MESSAGES)
    }

    sealed class Result {
        data class Ok(val list: AllowList) : Result()
        data class Refused(val code: String) : Result()
    }

    companion object {
        const val ROLE_EMERGENCY_CALL = "emergency_call"
        const val ROLE_DIALER = "dialer"
        const val ROLE_MESSAGES = "messages"
        const val MAX_CUSTOM_PACKAGES = 50

        private val SETTINGS_PREFIXES = listOf(
            "com.android.settings",
            "com.android.packageinstaller",
            "com.google.android.packageinstaller",
        )
        private val SETTINGS_PACKAGES = setOf(
            "com.google.android.permissioncontroller",
            "com.android.permissioncontroller",
        )
        private val SEGMENT = Regex("^[A-Za-z][A-Za-z0-9_]*$")

        fun parse(roles: List<String>, packages: List<String>): Result {
            var dialer = false
            var messages = false
            for (role in roles) {
                when (role) {
                    ROLE_EMERGENCY_CALL -> Unit
                    ROLE_DIALER -> dialer = true
                    ROLE_MESSAGES -> messages = true
                    else -> return Result.Refused("unknown_role")
                }
            }
            val accepted = sortedSetOf<String>()
            for (name in packages) {
                val error = packageError(name)
                if (error != null) return Result.Refused(error)
                accepted.add(name)
            }
            if (accepted.size > MAX_CUSTOM_PACKAGES) return Result.Refused("too_many_packages")
            return Result.Ok(AllowList(dialer, messages, accepted))
        }

        private fun packageError(name: String): String? {
            val segments = name.split('.')
            if (name.length > 255 || segments.size < 2 || !segments.all { SEGMENT.matches(it) }) return "invalid_package"
            val lowered = name.lowercase()
            if (lowered in SETTINGS_PACKAGES || SETTINGS_PREFIXES.any { lowered.startsWith(it) }) return "settings_package"
            return null
        }
    }
}
