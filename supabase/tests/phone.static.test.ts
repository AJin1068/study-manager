// Static checks of the S5.2/S5.3/S5.5 phone migration. They read the SQL as text, so they prove
// structure (RLS on every table, SELECT-only policies that test the unrevoked pair, definer
// functions pinned and revoked, codes hashed and rate limited by a counter that is not rolled
// back, lease length capped like the link-protocol crate) and NOT that Postgres accepts or
// enforces it: that needs `supabase test db` (tests/rls_s5_phone.sql) and is tracked as V5-2.

import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const migrationsDir = join(root, 'supabase', 'migrations');
const files = readdirSync(migrationsDir).filter((f) => f.endsWith('.sql') && f.includes('_s5_phone'));
const sql = files.map((f) => readFileSync(join(migrationsDir, f), 'utf8')).join('\n');
const code = sql
  .split('\n')
  .filter((line) => !line.trim().startsWith('--'))
  .join('\n');

type Rule = 'owner_or_paired_phone' | 'owner_or_phone' | 'none';
interface Matrix {
  tables: Record<string, Record<'select' | 'insert' | 'update' | 'delete', Rule>>;
}
interface Contract {
  rpcs: { name: string; params: string[]; returns: string[]; roles: string[] }[];
  internal: { name: string; params: string[] }[];
}
const matrix = JSON.parse(readFileSync(join(root, 'fixtures', 'phone', 'rls-matrix.json'), 'utf8')) as Matrix;
const contract = JSON.parse(readFileSync(join(root, 'fixtures', 'phone', 'rpc-contract.json'), 'utf8')) as Contract;

const tables = [...code.matchAll(/create table public\.(\w+)\s*\(/g)].map((m) => m[1]);
const starts = [...code.matchAll(/create or replace function public\.(\w+)\(/g)];
const blocks = starts.map((m, i) => ({ name: m[1], text: code.slice(m.index, starts[i + 1]?.index ?? code.length) }));
const block = (name: string) => blocks.find((b) => b.name === name)?.text ?? '';
const policies = code.split('create policy').slice(1).map((p) => p.slice(0, p.indexOf(';\n')));

describe('phone tables and row level security', () => {
  it('is exactly one migration that creates the tables of the matrix and only those', () => {
    expect(files).toHaveLength(1);
    expect([...tables].sort()).toEqual(Object.keys(matrix.tables).sort());
  });

  it('enables RLS and revokes every table privilege first', () => {
    for (const table of tables) {
      expect(code, table).toMatch(new RegExp(`alter table public\\.${table}\\s+enable row level security`));
    }
    const revoke = code.match(/revoke all on ([^;]*) from public, anon, authenticated;/)?.[1] ?? '';
    for (const table of tables) expect(revoke, table).toContain(`public.${table}`);
    expect(code).not.toMatch(/grant [^;]* to (anon|public)\b/);
  });

  it('grants and polices exactly what the matrix allows, and only SELECT', () => {
    for (const [table, ops] of Object.entries(matrix.tables)) {
      const grants = [...code.matchAll(new RegExp(`grant ([a-z, ]+) on public\\.${table} to authenticated;`, 'g'))].flatMap((m) =>
        m[1].split(',').map((s) => s.trim()),
      );
      const policy = policies.filter((p) => new RegExp(`\\bon public\\.${table}\\s`).test(p));
      if (ops.select === 'none') {
        expect(grants, table).toEqual([]);
        expect(policy, table).toEqual([]);
      } else {
        expect(grants, table).toEqual(['select']);
        expect(policy, table).toHaveLength(1);
        expect(policy[0], table).toMatch(/for select to authenticated/);
      }
      for (const op of ['insert', 'update', 'delete'] as const) expect(ops[op], `${table}.${op}`).toBe('none');
    }
    for (const policy of policies) expect(policy).not.toMatch(/for (insert|update|delete|all)\b/);
    expect(code).not.toMatch(/grant (insert|update|delete|all)/);
  });

  it('lets the owner or an UNREVOKED paired phone read, and nobody else', () => {
    for (const [table, ops] of Object.entries(matrix.tables)) {
      const policy = policies.find((p) => new RegExp(`\\bon public\\.${table}\\s`).test(p)) ?? '';
      if (ops.select === 'owner_or_paired_phone') {
        expect(policy, table).toContain('owner_user_id = auth.uid()');
        expect(policy, table).toContain('p.phone_user_id = auth.uid()');
        expect(policy, table).toContain('p.revoked_at is null');
        expect(policy, table).toContain(`p.owner_user_id = ${table}.owner_user_id`);
      }
      if (ops.select === 'owner_or_phone') {
        expect(policy, table).toContain('owner_user_id = auth.uid() or phone_user_id = auth.uid()');
      }
    }
  });

  it('allows one active phone per owner and one active owner per phone', () => {
    expect(code).toMatch(/create unique index phone_pairs_one_owner on public\.phone_pairs \(owner_user_id\) where revoked_at is null/);
    expect(code).toMatch(/create unique index phone_pairs_one_phone on public\.phone_pairs \(phone_user_id\) where revoked_at is null/);
    expect(code).toMatch(/check \(owner_user_id <> phone_user_id\)/);
  });

  it('has no column for app names, usage history, titles, URLs or typed text', () => {
    const forbidden = /^(title|window_title|url|typed\w*|app\w*|package\w*|usage\w*|history\w*|sentence\w*)$/i;
    const bodies = [...code.matchAll(/create table public\.\w+\s*\(([\s\S]*?)\n\);/g)].map((m) => m[1]);
    expect(bodies.length).toBe(tables.length);
    for (const body of bodies) {
      for (const line of body.split('\n')) {
        const column = line.trim().match(/^(\w+)\s/)?.[1];
        if (column && !['unique', 'primary', 'check'].includes(column)) expect(column, line).not.toMatch(forbidden);
      }
    }
  });

  it('constrains a lease like the link-protocol crate does', () => {
    const lease = readFileSync(join(root, 'crates', 'link-protocol', 'src', 'lease.rs'), 'utf8');
    const grace = Number(lease.match(/GRACE_MS: i64 = ([\d_]+);/)?.[1].replaceAll('_', ''));
    const segment = lease.match(/MAX_SEGMENT_MS: i64 = (\d+) \* (\d+) \* (\d+) \* (\d+);/)?.slice(1).map(Number) ?? [];
    const cap = segment.reduce((a, b) => a * b, 1) + grace;
    expect(cap).toBe(10_860_000);
    expect(code).toContain(`valid_until_ms - issued_at_ms <= ${cap}`);
    expect(block('publish_phone_lock')).toContain(`p_valid_until_ms - v_now > ${cap}`);
    expect(code).toMatch(/release_cause\s+text check \(release_cause in \('run_ended', 'emergency_pass', 'skip_today', 'early_release', 'pair_revoked'\)\)/);
  });
});

describe('phone security definer functions', () => {
  it('covers exactly the contract RPCs and the internal helper', () => {
    const expected = [...contract.rpcs.map((r) => r.name), ...contract.internal.map((r) => r.name)].sort();
    expect(blocks.map((b) => b.name).sort()).toEqual(expected);
  });

  it('pins search_path on every function', () => {
    for (const b of blocks) {
      expect(b.text, b.name).toContain('security definer');
      expect(b.text, b.name).toMatch(/set search_path = public, pg_temp/);
    }
  });

  it('grants EXECUTE to authenticated only, never to anon, and keeps the helper private', () => {
    for (const rpc of contract.rpcs) {
      expect(rpc.roles, rpc.name).toEqual(['authenticated']);
      const signature = `public\\.${rpc.name}\\([^)]*\\)`;
      const revoked = (code.match(new RegExp(`revoke execute on function ${signature} from ([a-z_, ]+);`))?.[1] ?? '').split(',').map((s) => s.trim());
      expect(revoked, `${rpc.name} revoke`).toEqual(expect.arrayContaining(['public', 'anon']));
      const grants = [...code.matchAll(new RegExp(`grant execute on function ${signature} to ([a-z_, ]+);`, 'g'))].flatMap((m) =>
        m[1].split(',').map((s) => s.trim()),
      );
      expect(grants, rpc.name).toEqual(['authenticated']);
    }
    for (const helper of contract.internal) {
      const revoke = code.match(new RegExp(`revoke execute on function public\\.${helper.name}\\([^)]*\\) from ([a-z_, ]+);`));
      expect((revoke?.[1] ?? '').split(',').map((s) => s.trim()), helper.name).toEqual(expect.arrayContaining(['public', 'anon', 'authenticated']));
      expect(code, helper.name).not.toMatch(new RegExp(`grant execute on function public\\.${helper.name}`));
    }
  });

  it('declares the contract parameters in order', () => {
    for (const rpc of [...contract.rpcs, ...contract.internal]) {
      const text = block(rpc.name);
      const header = text.slice(0, text.indexOf('returns'));
      expect([...header.matchAll(/\b(p_\w+)\s/g)].map((m) => m[1]), rpc.name).toEqual(rpc.params);
    }
  });

  it('reports refusals as returned codes, never by RAISE', () => {
    for (const b of blocks) expect(b.text, b.name).not.toMatch(/\braise\b/i);
    for (const rpc of contract.rpcs) {
      for (const value of [...block(rpc.name).matchAll(/'code', '(\w+)'/g)].map((m) => m[1])) {
        expect(rpc.returns, `${rpc.name} returns ${value}`).toContain(value);
      }
    }
  });

  it('identifies the caller with auth.uid(), never by a parameter, and finds the owner through the pair', () => {
    for (const rpc of contract.rpcs) expect(block(rpc.name), rpc.name).toContain('auth.uid()');
    for (const name of ['phone_lease', 'phone_beat', 'report_phone_pickups']) {
      expect(block(name), name).toContain('public.phone_owner_of(v_user)');
      expect(block(name), name).toMatch(/'not_paired'/);
    }
    expect(block('phone_owner_of')).toContain('revoked_at is null');
    for (const rpc of contract.rpcs) expect(rpc.params.filter((p) => /owner|user/.test(p)), rpc.name).toEqual([]);
  });

  it('makes codes single-use, 5 minutes long, hashed and from the server CSPRNG', () => {
    const make = block('create_pair_code');
    expect(make).toContain("interval '5 minutes'");
    expect(make).toContain('gen_random_uuid()');
    expect(make).toContain("encode(sha256(convert_to(v_code, 'UTF8')), 'hex')");
    expect(make).toMatch(/'anonymous_not_allowed'/);
    expect(code).toMatch(/code_hash\s+text not null unique/);
    expect(code).not.toMatch(/pair_codes[\s\S]{0,200}\bcode\s+text/);
    const claim = block('claim_pair');
    expect(claim).toContain('used_at is not null');
    expect(claim).toContain('expires_at <= now()');
    expect(claim).toMatch(/update public\.pair_codes set used_at = now\(\) where id = v_row\.id/);
    expect(claim).toContain('for update');
    expect(claim).toContain('v_row.owner_user_id = v_user');
  });

  it('counts a claim attempt before checking anything, so a refusal cannot roll it back', () => {
    const claim = block('claim_pair');
    expect(claim).toContain("interval '1 minute'");
    expect(claim).toMatch(/>= 5/);
    expect(claim.indexOf('insert into public.pair_attempts')).toBeGreaterThan(0);
    expect(claim.indexOf('insert into public.pair_attempts')).toBeLessThan(claim.indexOf("'invalid_code'"));
    expect(claim.indexOf("'rate_limited'")).toBeLessThan(claim.indexOf('insert into public.pair_attempts'));
  });

  it('refuses a stale or repeated sequence number and a lease that is too long', () => {
    const publish = block('publish_phone_lock');
    expect(publish).toContain('p_seq < v_cur.seq');
    expect(publish).toContain('p_seq = v_cur.seq');
    expect(publish).toMatch(/'stale_seq'/);
    expect(publish).toMatch(/'lease_too_long'/);
    expect(publish).toContain('for update');
  });

  it('lets either side revoke by setting revoked_at, which every read path tests', () => {
    expect(block('revoke_pair')).toContain('set revoked_at = now()');
    expect(block('revoke_pair')).toContain('owner_user_id = v_user or phone_user_id = v_user');
  });

  it('keeps pick-up reports idempotent and bounded', () => {
    const report = block('report_phone_pickups');
    expect(report).toContain('on conflict (owner_user_id, segment_id, count) do nothing');
    expect(report).toContain('p_count not between 0 and 100000');
    expect(code).toMatch(/unique \(owner_user_id, segment_id, count\)/);
  });

  it('contains no service-role credential', () => {
    expect(sql).not.toMatch(/service_role_key|SERVICE_ROLE_KEY/);
  });
});
