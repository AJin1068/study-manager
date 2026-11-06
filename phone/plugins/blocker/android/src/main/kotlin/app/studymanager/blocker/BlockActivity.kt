package app.studymanager.blocker

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.Gravity
import android.view.ViewGroup
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView

/**
 * The full-screen block screen: the time left and a way home. It shows no app name, and reading
 * this screen tells nothing about which app was blocked. Built in code so it needs no layout
 * resources; the design pass can replace it.
 */
class BlockActivity : Activity() {
    private val ticker = Handler(Looper.getMainLooper())
    private lateinit var remaining: TextView

    private val tick = object : Runnable {
        override fun run() {
            if (!LockController.isLocked()) {
                finish()
                return
            }
            remaining.text = getString(R.string.blocker_remaining, format(LockController.remainingMs()))
            ticker.postDelayed(this, 1000)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
            layoutParams = ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
            setPadding(48, 48, 48, 48)
        }
        column.addView(TextView(this).apply { text = getString(R.string.blocker_title); textSize = 24f; gravity = Gravity.CENTER })
        remaining = TextView(this).apply { textSize = 18f; gravity = Gravity.CENTER }
        column.addView(remaining)
        column.addView(TextView(this).apply { text = getString(R.string.blocker_emergency_hint); gravity = Gravity.CENTER })
        column.addView(Button(this).apply {
            text = getString(R.string.blocker_home)
            setOnClickListener { goHome() }
        })
        setContentView(column)
    }

    override fun onResume() {
        super.onResume()
        tick.run()
    }

    override fun onPause() {
        ticker.removeCallbacks(tick)
        super.onPause()
    }

    @Deprecated("The back button goes home like the button does, never back into a blocked app.")
    override fun onBackPressed() = goHome()

    private fun goHome() {
        startActivity(Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_HOME).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        finish()
    }

    private fun format(ms: Long): String {
        val total = ms / 1000
        return "%d:%02d".format(total / 60, total % 60)
    }
}
