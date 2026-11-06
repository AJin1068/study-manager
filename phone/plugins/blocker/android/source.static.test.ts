// Static checks of the Kotlin blocker plugin source (phone/plugins/blocker/android/). They read
// text only: the source has never been compiled or run here (V5-4, V5-5).

import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..', '..');
const plugin = join(root, 'phone', 'plugins', 'blocker', 'android');
const read = (...parts: string[]) => readFileSync(join(plugin, ...parts), 'utf8');
const kotlinDir = join(plugin, 'src', 'main', 'kotlin', 'app', 'studymanager', 'blocker');
const kotlin = readdirSync(kotlinDir).map((f: string) => ({ name: f, text: readFileSync(join(kotlinDir, f), 'utf8') }));
const fixture = JSON.parse(readFileSync(join(root, 'fixtures', 'phone', 'blocker-bridge.json'), 'utf8')) as {
  commands: Record<string, unknown>;
  events: { event: { event: string } }[];
};

describe('blocker plugin source', () => {
  it('declares the accessibility service, the block activity and the boot receiver', () => {
    const manifest = read('src', 'main', 'AndroidManifest.xml');
    expect(manifest).toContain('android.permission.BIND_ACCESSIBILITY_SERVICE');
    expect(manifest).toContain('android:name=".BlockerAccessibilityService"');
    expect(manifest).toContain('android:name=".BlockActivity"');
    expect(manifest).toContain('android.intent.action.BOOT_COMPLETED');
  });

  it('has no network permission and reads no window content', () => {
    expect(read('src', 'main', 'AndroidManifest.xml')).not.toMatch(/INTERNET|PACKAGE_USAGE_STATS|QUERY_ALL_PACKAGES/);
    const config = read('src', 'main', 'res', 'xml', 'blocker_accessibility_service.xml');
    expect(config).toContain('android:canRetrieveWindowContent="false"');
    expect(config).toContain('typeWindowStateChanged');
  });

  it('never logs, stores or sends the package in front', () => {
    for (const { name, text } of kotlin) {
      expect(text, name).not.toMatch(/\bLog\.|println\(|UsageStatsManager|HttpURLConnection|OkHttp/);
    }
    const service = kotlin.find((f) => f.name === 'BlockerAccessibilityService.kt')?.text ?? '';
    // `pkg` is compared with the allowed set and dropped: it appears nowhere else.
    expect(service.match(/\bpkg\b/g)?.length).toBe(2);
  });

  it('keeps the emergency call and refuses the settings app', () => {
    const allow = kotlin.find((f) => f.name === 'AllowList.kt')?.text ?? '';
    expect(allow).toContain('val allowsEmergencyCall: Boolean get() = true');
    expect(allow).toContain('add(ROLE_EMERGENCY_CALL)');
    expect(allow).toContain('"com.android.settings"');
  });

  it('measures the lock end on the monotonic clock', () => {
    const controller = kotlin.find((f) => f.name === 'LockController.kt')?.text ?? '';
    expect(controller).toContain('SystemClock.elapsedRealtime()');
  });

  it('implements every command and event of the shared contract', () => {
    const plugin = kotlin.find((f) => f.name === 'BlockerPlugin.kt')?.text ?? '';
    const events = kotlin.find((f) => f.name === 'BlockerEvents.kt')?.text ?? '';
    for (const command of Object.keys(fixture.commands)) expect(plugin, command).toContain(`fun ${command}(invoke: Invoke)`);
    for (const { event } of fixture.events) expect(events, event.event).toContain(`"${event.event}"`);
  });
});
