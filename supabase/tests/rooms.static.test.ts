// Static checks of the stage-4 room migration. They read the SQL as text, so they prove structure
// (RLS on, definer functions pinned and revoked, grants matching the shared contract, no RAISE for
// coded failures, no column for private content) and NOT that Postgres accepts or enforces it:
// that needs `supabase test db` (tests/rls_s4.sql) and is tracked as V4-1.

import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const migrationsDir = join(root, 'supabase', 'migrations');
const files = readdirSync(migrationsDir).filter((f) => f.endsWith('.sql') && f.includes('_s4_'));
const sql = files.map((f) => readFileSync(join(migrationsDir, f), 'utf8')).join('\n');
const code = sql
  .split('\n')
  .filter((line) => !line.trim().startsWith('--'))
  .join('\n');

type Rule = 'owner' | 'none';
interface Matrix {
  tables: Record<string, Record<'select' | 'insert' | 'update' | 'delete', Rule>>;
}
interface Contract {
  rpcs: { name: string; params: string[]; returns: string[]; roles: string[] }[];
  internal: { name: string; params: string[] }[];
}
const matrix = JSON.parse(readFileSync(join(root, 'fixtures', 'rooms', 'rls-matrix.json'), 'utf8')) as Matrix;
const contract = JSON.parse(readFileSync(join(root, 'fixtures', 'rooms', 'rpc-contract.json'), 'utf8')) as Contract;

const tables = [...code.matchAll(/create table public\.(\w+)\s*\(/g)].map((m) => m[1]);
const starts = [...code.matchAll(/create or replace function public\.(\w+)\(/g)];
const blocks = starts.map((m, i) => ({ name: m[1], text: code.slice(m.index, starts[i + 1]?.index ?? code.length) }));

describe('stage-4 row level security', () => {
  it('exists as exactly one migration', () => {
    expect(files).toHaveLength(1);
  });

  it('creates the tables the shared matrix describes, and only those', () => {
    expect([...tables].sort()).toEqual(Object.keys(matrix.tables).sort());
  });

  it('enables RLS on every table', () => {
    for (const table of tables) {
      expect(code, table).toMatch(new RegExp(`alter table public\\.${table}\\s+enable row level security`));
    }
  });

  it('has a select policy only where the matrix says owner, and never an insert/update/delete policy', () => {
    for (const [table, ops] of Object.entries(matrix.tables)) {
      for (const [op, rule] of Object.entries(ops)) {
        const policy = new RegExp(`create policy \\w+ on public\\.${table}\\s+for ${op}\\b`);
        if (rule === 'owner') expect(code, `${table}.${op}`).toMatch(policy);
        else expect(code, `${table}.${op}`).not.toMatch(policy);
      }
    }
  });

  it('scopes every policy to the signed-in owner', () => {
    const policies = code.split('create policy').slice(1);
    expect(policies.length).toBeGreaterThan(0);
    for (const policy of policies) {
      const statement = policy.slice(0, policy.indexOf(';'));
      expect(statement).toContain('to authenticated');
      expect(statement).toContain('user_id = auth.uid()');
    }
  });

  it('revokes every table privilege and grants only what the matrix allows', () => {
    const revoke = code.match(/revoke all on ([^;]*) from public, anon, authenticated;/)?.[1] ?? '';
    for (const table of tables) expect(revoke, table).toContain(`public.${table}`);
    for (const [table, ops] of Object.entries(matrix.tables)) {
      const granted = [...code.matchAll(new RegExp(`grant ([a-z, ]+) on public\\.${table}\\s+to authenticated;`, 'g'))]
        .flatMap((m) => m[1].split(',').map((s) => s.trim()))
        .sort();
      const allowed = Object.entries(ops)
        .filter(([, rule]) => rule === 'owner')
        .map(([op]) => op)
        .sort();
      expect(granted, table).toEqual(allowed);
    }
    expect(code).not.toMatch(/grant [^;]* to (anon|public)\b/);
  });
});

describe('stage-4 security definer functions', () => {
  it('covers exactly the contract RPCs and the internal helpers', () => {
    const expected = [...contract.rpcs.map((r) => r.name), ...contract.internal.map((r) => r.name)].sort();
    expect(blocks.map((b) => b.name).sort()).toEqual(expected);
  });

  it('pins search_path on every function', () => {
    for (const block of blocks) {
      expect(block.text, block.name).toContain('security definer');
      expect(block.text, block.name).toMatch(/set search_path = public, pg_temp/);
    }
  });

  it('revokes EXECUTE from public and anon, then grants only authenticated', () => {
    for (const rpc of contract.rpcs) {
      const signature = `public\\.${rpc.name}\\([^)]*\\)`;
      const revoke = code.match(new RegExp(`revoke execute on function ${signature} from ([a-z_, ]+);`));
      expect(revoke, `${rpc.name} revoke`).not.toBeNull();
      expect((revoke?.[1] ?? '').split(',').map((s) => s.trim()), rpc.name).toEqual(expect.arrayContaining(['public', 'anon']));
      const grants = [...code.matchAll(new RegExp(`grant execute on function ${signature} to ([a-z_, ]+);`, 'g'))].flatMap((m) =>
        m[1].split(',').map((s) => s.trim()),
      );
      expect(grants.sort(), rpc.name).toEqual([...rpc.roles].sort());
    }
  });

  it('keeps the internal helpers away from every client role', () => {
    for (const helper of contract.internal) {
      const revoke = code.match(new RegExp(`revoke execute on function public\\.${helper.name}\\([^)]*\\) from ([a-z_, ]+);`));
      const revoked = (revoke?.[1] ?? '').split(',').map((s) => s.trim());
      expect(revoked, helper.name).toEqual(expect.arrayContaining(['public', 'anon', 'authenticated']));
      expect(code, helper.name).not.toMatch(new RegExp(`grant execute on function public\\.${helper.name}`));
    }
  });

  it('declares the contract parameters in order', () => {
    for (const rpc of [...contract.rpcs, ...contract.internal]) {
      const block = blocks.find((b) => b.name === rpc.name);
      const header = block?.text.slice(0, block.text.indexOf('returns')) ?? '';
      expect([...header.matchAll(/\b(p_\w+)\s/g)].map((m) => m[1]), rpc.name).toEqual(rpc.params);
    }
  });

  it('reports refusals as returned codes, never by RAISE (a rollback would erase the attempt record)', () => {
    for (const block of blocks) {
      expect(block.text, block.name).not.toMatch(/\braise\b/i);
    }
    for (const rpc of contract.rpcs) {
      const block = blocks.find((b) => b.name === rpc.name);
      const returned = [...(block?.text ?? '').matchAll(/'code', '(\w+)'/g)].map((m) => m[1]);
      for (const value of returned) expect(rpc.returns, `${rpc.name} returns ${value}`).toContain(value);
    }
  });

  it('identifies the caller with auth.uid() in every RPC', () => {
    for (const rpc of contract.rpcs) {
      expect(blocks.find((b) => b.name === rpc.name)?.text, rpc.name).toContain('auth.uid()');
    }
  });

  it('never takes a user id for the caller as a parameter (other-user ids only for kick)', () => {
    for (const rpc of contract.rpcs) {
      const userParams = rpc.params.filter((p) => /user/.test(p));
      expect(userParams, rpc.name).toEqual(rpc.name === 'kick_member' ? ['p_target_user'] : []);
    }
  });
});

describe('what the room schema may hold', () => {
  it('has no column for titles, URLs, typed text, or the character inputs', () => {
    const forbidden = /^(title|window_title|url|typed\w*|sentence\w*|app_name|mbti|birth\w*|gender|level_xp)$/i;
    const bodies = [...code.matchAll(/create table public\.\w+\s*\(([\s\S]*?)\n\);/g)].map((m) => m[1]);
    expect(bodies.length).toBe(tables.length);
    for (const body of bodies) {
      for (const line of body.split('\n')) {
        const column = line.trim().match(/^(\w+)\s/)?.[1];
        if (column) expect(column, line).not.toMatch(forbidden);
      }
    }
  });

  it('stores only a hash of the invite code', () => {
    const body = code.match(/create table public\.room_invites\s*\(([\s\S]*?)\n\);/)?.[1] ?? '';
    expect(body).toMatch(/\bcode_hash\b/);
    expect(body).not.toMatch(/\bcode\s+text\b/);
  });

  it('contains no service-role credential', () => {
    expect(sql).not.toMatch(/service_role_key|SERVICE_ROLE_KEY/);
  });
});
