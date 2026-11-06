package app.studymanager.blocker

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** Reads the same fixture as the Rust and TypeScript tests (`fixtures/phone/blocker-bridge.json`). Not run here. */
class AllowListTest {
    private val allowList: JSONObject = JSONObject(
        javaClass.classLoader!!.getResourceAsStream("blocker-bridge.json")!!.bufferedReader().readText(),
    ).getJSONObject("allowList")

    private fun strings(array: JSONArray) = List(array.length()) { array.getString(it) }

    @Test
    fun validInputsNormalizeToTheSharedWireForm() {
        val cases = allowList.getJSONArray("valid")
        for (i in 0 until cases.length()) {
            val case = cases.getJSONObject(i)
            val input = case.getJSONObject("input")
            val result = AllowList.parse(strings(input.getJSONArray("roles")), strings(input.getJSONArray("packages")))
            val list = (result as AllowList.Result.Ok).list
            val wire = case.getJSONObject("wire")
            assertEquals(case.getString("name"), strings(wire.getJSONArray("roles")), list.toWireRoles())
            assertEquals(case.getString("name"), strings(wire.getJSONArray("packages")), list.packages.toList())
            assertTrue(list.allowsEmergencyCall)
        }
    }

    @Test
    fun refusedInputsCarryTheSharedCodes() {
        val cases = allowList.getJSONArray("invalid")
        for (i in 0 until cases.length()) {
            val case = cases.getJSONObject(i)
            val input = case.get("input") as? JSONObject ?: continue
            val roles = input.optJSONArray("roles") ?: continue
            val packages = input.optJSONArray("packages") ?: continue
            if (case.getString("error") == "malformed") continue // shape errors are caught by the plugin's argument parsing
            val result = AllowList.parse(strings(roles), strings(packages))
            assertEquals(case.getString("name"), case.getString("error"), (result as AllowList.Result.Refused).code)
        }
    }
}
