// Static checks of the anonymous-account guard migration. They read the SQL as text, so they prove
// that the guarded list is exactly "every RPC an authenticated caller can run, minus what the phone
// needs" and NOT that Postgres accepts it: that needs `supabase test db`
// (tests/rls_s5_anonymous_guard.sql) and is tracked as V5-2. (The migration and that test were also
// run once against a plain Postgres 16 with a stubbed `auth` schema; that is not the Supabase stack.)

import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const migrationsDir = join(root, 'supabase', 'migrations');
const files = readdirSync(migrationsDir).filter((f) => f.endsWith('_anonymous_guard.sql'));
const sql = files.map((f) => readFileSync(join(migrationsDir, f), 'utf8')).join('\n');
const code = sql
  .split('\n')
  .filter((line) => !line.trim().startsWith('--'))
  .join('\n');

interface Rule {
  refusalCode: string;
  allowed: string[];
  ownGuard: string[];
  textReturning: string[];
}
interface Contract {
  rpcs: { name: string; roles: string[] }[];
}
const rule = JSON.parse(readFileSync(join(root, 'fixtures', 'phone', 'anonymous-callers.json'), 'utf8')) as Rule;
const contracts = ['sync', 'rooms', 'shared_wiki', 'profile', 'phone'].map(
  (dir) => JSON.parse(readFileSync(join(root, 'fixtures', dir, 'rpc-contract.json'), 'utf8')) as Contract,
);
const authenticatedRpcs = contracts.flatMap((c) => c.rpcs.filter((r) => r.roles.includes('authenticated')).map((r) => r.name));

// The signature lists of the two `foreach` loops: jsonb RPCs first, text RPCs second.
const arrays = [...code.matchAll(/foreach v_signature in array array\[([\s\S]*?)\]\s*loop/g)].map((m) =>
  [...m[1].matchAll(/'public\.(\w+)\(([^']*)\)'/g)].map((s) => s[1]),
);
const [jsonbGuarded = [], textGuarded = []] = arrays;

describe('anonymous (phone) accounts and owner-side RPCs', () => {
  it('is exactly one migration with a jsonb list and a text list', () => {
    expect(files).toHaveLength(1);
    expect(arrays).toHaveLength(2);
  });

  it('guards every authenticated RPC except the phone side, the public profile and create_pair_code', () => {
    const expected = authenticatedRpcs.filter((n) => !rule.allowed.includes(n) && !rule.ownGuard.includes(n)).sort();
    expect([...jsonbGuarded, ...textGuarded].sort()).toEqual(expected);
  });

  it('never guards what the phone needs', () => {
    for (const name of rule.allowed) expect([...jsonbGuarded, ...textGuarded], name).not.toContain(name);
  });

  it('returns text for the sync RPCs and jsonb for the rest', () => {
    expect([...textGuarded].sort()).toEqual([...rule.textReturning].sort());
    expect(code).toMatch(/return jsonb_build_object\(''code'', ''anonymous_not_allowed''\)/);
    expect(code).toMatch(/return ''anonymous_not_allowed''/);
    expect(code).toContain(`'${rule.refusalCode}'`);
  });

  it('reads the claim the way create_pair_code does and fails the migration if a guard did not go in', () => {
    expect(code).toContain("coalesce((auth.jwt() ->> ''is_anonymous'')::boolean, false)");
    const phone = readFileSync(join(migrationsDir, readdirSync(migrationsDir).find((f) => f.includes('_s5_phone'))!), 'utf8');
    expect(phone).toContain("coalesce((auth.jwt() ->> 'is_anonymous')::boolean, false)");
    expect(code).toMatch(/if v_guarded = v_definition then\s+raise exception/);
  });

  it('changes no grant and creates no table', () => {
    expect(code).not.toMatch(/\bgrant\b|\brevoke\b|create table|create policy/i);
  });

  it('keeps the pgTAP plan and the guarded-function count in step with the list', () => {
    const pgtap = readFileSync(join(root, 'supabase', 'tests', 'rls_s5_anonymous_guard.sql'), 'utf8');
    expect(pgtap).toContain(`${jsonbGuarded.length + textGuarded.length}, 'the guard is in ${jsonbGuarded.length + textGuarded.length} functions'`);
    expect(pgtap).toContain(`'${rule.refusalCode}'`);
  });
});
