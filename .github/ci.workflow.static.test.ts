// Static checks of how the shared link-protocol crate is wired in. They read files as text: they
// prove the wiring (path dependency only, no root Cargo workspace, a CI step) and NOT that CI runs.

import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const read = (...parts: string[]) => readFileSync(join(root, ...parts), 'utf8');

describe('link-protocol wiring', () => {
  it('is an independent crate that src-tauri uses only as a path dependency', () => {
    expect(existsSync(join(root, 'Cargo.toml'))).toBe(false);
    expect(read('crates', 'link-protocol', 'Cargo.toml')).not.toContain('[workspace]');
    expect(existsSync(join(root, 'crates', 'link-protocol', 'Cargo.lock'))).toBe(true);
    expect(read('src-tauri', 'Cargo.toml')).toMatch(/^link-protocol = \{ path = "\.\.\/crates\/link-protocol" \}$/m);
  });

  it('has no dependency beyond serde and serde_json', () => {
    const deps = read('crates', 'link-protocol', 'Cargo.toml').split('[dependencies]')[1] ?? '';
    expect([...deps.matchAll(/^(\w[\w-]*) =/gm)].map((m) => m[1]).sort()).toEqual(['serde', 'serde_json']);
  });

  it('tests the crate in CI next to the desktop tests', () => {
    const ci = read('.github', 'workflows', 'ci.yml');
    const step = ci.split('- name: Link protocol crate')[1] ?? '';
    expect(step).toContain('working-directory: crates/link-protocol');
    expect(step).toContain('cargo test');
    expect(ci).toContain('crates/link-protocol');
  });
});
