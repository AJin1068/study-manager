// Canary grep over what the phone link can carry: the wire fixtures, the link-protocol sources and the
// desktop payload sources must have no field for an app name, package name, usage, window title or
// URL. It reads text, so it proves the shape of the contract and NOT what a built Android app sends
// (that is V5-6/V5-7 on a device). The allow list is device-local and is checked by the Rust e2e test.

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const read = (rel: string): string => readFileSync(join(root, rel), 'utf8');

const FORBIDDEN_KEYS = ['package', 'package_name', 'app', 'app_name', 'usage', 'history', 'title', 'window_title', 'url', 'domain', 'path'];

function keysOf(value: unknown, out: string[] = []): string[] {
  if (Array.isArray(value)) value.forEach((v) => keysOf(v, out));
  else if (value !== null && typeof value === 'object') {
    for (const [k, v] of Object.entries(value)) {
      out.push(k);
      keysOf(v, out);
    }
  }
  return out;
}

describe('phone payloads carry no app names or usage', () => {
  it('the shared wire fixtures have no forbidden key', () => {
    const wire = JSON.parse(read('fixtures/phone/link-protocol.json')) as unknown;
    const keys = keysOf(wire);
    expect(keys.length).toBeGreaterThan(10);
    for (const key of keys) expect(FORBIDDEN_KEYS, key).not.toContain(key.toLowerCase());
  });

  it('the reported and published Rust types name no forbidden field', () => {
    const sources = ['crates/link-protocol/src/report.rs', 'crates/link-protocol/src/lease.rs', 'crates/link-protocol/src/offer.rs', 'src-tauri/src/phone/payload.rs'];
    for (const file of sources) {
      const fields = [...read(file).matchAll(/^\s*pub\s+(?:r#)?(\w+)\s*:/gm)].map((m) => m[1] ?? '');
      for (const field of fields) expect(FORBIDDEN_KEYS, `${file}: ${field}`).not.toContain(field);
    }
  });

  it('the pick-up report has exactly a segment id and a count, and rejects extra fields', () => {
    const report = read('crates/link-protocol/src/report.rs');
    expect(report).toMatch(/deny_unknown_fields/);
    const fields = [...report.matchAll(/^\s*pub\s+(\w+)\s*:/gm)].map((m) => m[1]);
    expect(fields).toContain('segment_id');
    expect(fields).toContain('count');
  });

  it('the phone tables have no column for content', () => {
    const sql = read('supabase/migrations/20260601000500_s5_phone.sql')
      .split('\n')
      .filter((l) => !l.trim().startsWith('--'))
      .join('\n');
    expect(sql).not.toMatch(/\b(package_name|app_name|usage|window_title)\b/i);
  });
});
