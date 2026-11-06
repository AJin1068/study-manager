// Static checks of the phone project layout. They read text only: nothing here proves the project
// builds or runs on Android (V5-1, V5-4).

import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const read = (rel: string) => readFileSync(join(root, rel), 'utf8');

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    if (['target', 'node_modules', 'gen', 'dist', '.git', '.gjc'].includes(name)) continue;
    const full = join(dir, name);
    if (statSync(full).isDirectory()) walk(full, out);
    else out.push(full);
  }
  return out;
}

describe('phone project layout', () => {
  it('is independent: no root workspace, no dependency on src-tauri, only the shared wire crate', () => {
    expect(existsSync(join(root, 'Cargo.toml'))).toBe(false);
    for (const manifest of ['phone/core/Cargo.toml', 'phone/src-tauri/Cargo.toml']) {
      const text = read(manifest);
      expect(text, manifest).not.toMatch(/path\s*=\s*"(\.\.\/)+src-tauri/);
      expect(text, manifest).not.toMatch(/objc2|keyring|rusqlite/);
      expect(text, manifest).not.toContain('[workspace]');
    }
    expect(read('phone/core/Cargo.toml')).toContain('path = "../../crates/link-protocol"');
    expect(read('phone/src-tauri/Cargo.toml')).toContain('path = "../core"');
  });

  it('has no Swift source and no iOS block of its own', () => {
    const swift = walk(join(root, 'phone')).filter((f) => f.endsWith('.swift'));
    expect(swift).toEqual([]);
    expect(read('phone/core/src/platform.rs')).toContain('#[cfg(target_os = "ios")]');
  });

  it('gives the webview no network, filesystem or shell permission', () => {
    const capability = JSON.parse(read('phone/src-tauri/capabilities/default.json')) as { permissions: string[] };
    for (const permission of capability.permissions) expect(permission, permission).not.toMatch(/^(http|fs|shell|opener|os):/);
    expect(read('phone/src-tauri/tauri.conf.json')).toContain("default-src 'self'");
  });

  it('declares the plugin with the identifiers the Kotlin class has', () => {
    expect(read('phone/src-tauri/src/lib.rs')).toContain('register_android_plugin("app.studymanager.blocker", "BlockerPlugin")');
    expect(read('phone/plugins/blocker/android/src/main/kotlin/app/studymanager/blocker/BlockerPlugin.kt')).toContain('class BlockerPlugin(');
    expect(read('phone/src-tauri/src/bridge.rs')).toContain('run_mobile_plugin');
  });

  it('the screens name no app, package or usage field', () => {
    for (const file of walk(join(root, 'phone', 'src')).filter((f) => f.endsWith('.tsx') || f.endsWith('api.ts'))) {
      expect(readFileSync(file, 'utf8'), file).not.toMatch(/package_name|app_name|window_title|usageStats/i);
    }
  });
});
