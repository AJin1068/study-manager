// Static checks of the shared-wiki migration (S4.7). They read the SQL as text, so they prove
// structure (RLS on with no client policy, definer functions pinned and revoked, consent and
// credit enforced in the merge RPC, no RAISE) and NOT that Postgres accepts or enforces it: that
// needs `supabase test db` (tests/rls_s4_shared_wiki.sql) and is tracked as V4-1.

import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const migrationsDir = join(root, 'supabase', 'migrations');
const files = readdirSync(migrationsDir).filter((f) => f.endsWith('_shared_wiki.sql'));
const sql = files.map((f) => readFileSync(join(migrationsDir, f), 'utf8')).join('\n');
const code = sql
  .split('\n')
  .filter((line) => !line.trim().startsWith('--'))
  .join('\n');

interface Matrix {
  tables: Record<string, Record<'select' | 'insert' | 'update' | 'delete', 'none'>>;
}
interface Contract {
  rpcs: { name: string; params: string[]; returns: string[]; roles: string[] }[];
  internal: { name: string; params: string[] }[];
}
const matrix = JSON.parse(readFileSync(join(root, 'fixtures', 'shared_wiki', 'rls-matrix.json'), 'utf8')) as Matrix;
const contract = JSON.parse(readFileSync(join(root, 'fixtures', 'shared_wiki', 'rpc-contract.json'), 'utf8')) as Contract;

const tables = [...code.matchAll(/create table public\.(\w+)\s*\(/g)].map((m) => m[1]);
const starts = [...code.matchAll(/create or replace function public\.(\w+)\(/g)];
const blocks = starts.map((m, i) => ({ name: m[1], text: code.slice(m.index, starts[i + 1]?.index ?? code.length) }));
const block = (name: string) => blocks.find((b) => b.name === name)?.text ?? '';

describe('shared wiki row level security', () => {
  it('exists as exactly one migration', () => {
    expect(files).toHaveLength(1);
  });

  it('creates the tables the shared matrix describes, and only those', () => {
    expect([...tables].sort()).toEqual(Object.keys(matrix.tables).sort());
  });

  it('enables RLS on every table and defines no client policy at all', () => {
    for (const table of tables) {
      expect(code, table).toMatch(new RegExp(`alter table public\\.${table}\\s+enable row level security`));
    }
    expect(code).not.toMatch(/create policy/);
  });

  it('revokes every table privilege and grants none', () => {
    const revoke = code.match(/revoke all on ([^;]*) from public, anon, authenticated;/)?.[1] ?? '';
    for (const table of tables) expect(revoke, table).toContain(`public.${table}`);
    expect(code).not.toMatch(/grant [^;]* on public\.\w+\s+to/);
    expect(code).not.toMatch(/grant [^;]* to (anon|public)\b/);
  });
});

describe('shared wiki security definer functions', () => {
  it('covers exactly the contract RPCs', () => {
    expect(blocks.map((b) => b.name).sort()).toEqual([...contract.rpcs.map((r) => r.name), ...contract.internal.map((r) => r.name)].sort());
  });

  it('pins search_path on every function', () => {
    for (const b of blocks) {
      expect(b.text, b.name).toContain('security definer');
      expect(b.text, b.name).toMatch(/set search_path = public, pg_temp/);
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

  it('declares the contract parameters in order', () => {
    for (const rpc of contract.rpcs) {
      const text = block(rpc.name);
      const header = text.slice(0, text.indexOf('returns'));
      expect([...header.matchAll(/\b(p_\w+)\s/g)].map((m) => m[1]), rpc.name).toEqual(rpc.params);
    }
  });

  it('reports refusals as returned codes, never by RAISE, and only codes in the contract', () => {
    for (const b of blocks) expect(b.text, b.name).not.toMatch(/\braise\b/i);
    for (const rpc of contract.rpcs) {
      const returned = [...block(rpc.name).matchAll(/'code', '(\w+)'/g)].map((m) => m[1]);
      expect(returned.length, rpc.name).toBeGreaterThan(0);
      for (const value of returned) expect(rpc.returns, `${rpc.name} returns ${value}`).toContain(value);
    }
  });

  it('identifies the caller with auth.uid() and checks room membership in every RPC', () => {
    for (const rpc of contract.rpcs) {
      expect(block(rpc.name), rpc.name).toContain('auth.uid()');
      expect(block(rpc.name), rpc.name).toContain('is_room_member(');
    }
  });

  it('never takes a user id as a parameter', () => {
    for (const rpc of contract.rpcs) expect(rpc.params.filter((p) => /user/.test(p)), rpc.name).toEqual([]);
  });
});

describe('shared wiki consent and credit', () => {
  it('merge consent is off unless the author turned it on, and is an explicit argument', () => {
    expect(code).toMatch(/allow_llm_merge boolean not null default false/);
    expect(block('publish_shared_page')).toContain('p_allow_llm_merge is null');
  });

  it('a merge accepts only live, consented edits of members, and derives the credit itself', () => {
    const text = block('save_merged_page');
    expect(text).toContain("r.kind = 'edit'");
    expect(text).toContain('r.withdrawn_at is null');
    expect(text).toContain('r.allow_llm_merge');
    expect(text).toContain('is_room_member(p_room_id, r.user_id)');
    expect(text).toContain("'not_consented'");
    expect(text).toMatch(/array_agg\(distinct r\.user_id\)/);
    expect(contract.rpcs.find((r) => r.name === 'save_merged_page')?.params.some((p) => /contrib/.test(p))).toBe(false);
  });

  it('revisions are append-only: no RPC deletes a revision, and withdrawing empties the body', () => {
    expect(code).not.toMatch(/delete\s+from\s+public\.shared_page_revisions/i);
    expect(block('withdraw_shared_page')).toMatch(/set body = '', withdrawn_at = now\(\)/);
    expect(code).toMatch(/check \(\(withdrawn_at is null and length\(body\) >= 1\) or \(withdrawn_at is not null and body = ''\)\)/);
  });

  it('a merge that credits a withdrawn or departed member is not listed', () => {
    const text = block('list_shared_pages');
    expect(text).toContain('withdrawn_at is null');
    expect(text).toMatch(/not exists \(select 1 from unnest\(r\.contributor_ids\)/);
  });

  it('the size limits are in the table and in the RPC', () => {
    expect(code).toMatch(/length\(body\) <= 20000/);
    expect(code).toMatch(/length\(title\) between 1 and 80/);
    expect(block('publish_shared_page')).toContain('20000');
  });

  it('no column holds anything but the shared text, its author and its bookkeeping', () => {
    const forbidden = /^(window_title|url|typed\w*|sentence\w*|app_name|mbti|birth\w*|gender|deviation\w*|distraction\w*)$/i;
    const bodies = [...code.matchAll(/create table public\.\w+\s*\(([\s\S]*?)\n\);/g)].map((m) => m[1]);
    expect(bodies.length).toBe(tables.length);
    for (const body of bodies) {
      for (const line of body.split('\n')) {
        const column = line.trim().match(/^(\w+)\s/)?.[1];
        if (column) expect(column, line).not.toMatch(forbidden);
      }
    }
  });

  it('contains no service-role credential', () => {
    expect(sql).not.toMatch(/service_role_key|SERVICE_ROLE_KEY/);
  });
});
