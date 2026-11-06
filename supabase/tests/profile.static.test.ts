// Static checks of the S4.9/S4.10 profile and gift migration. They read the SQL as text, so they
// prove structure (RLS on with no policy, definer functions pinned and revoked, only
// get_public_profile open to anonymous callers, scopes default to private, no column for deviation
// statistics) and NOT that Postgres accepts or enforces it: that needs `supabase test db`
// (tests/rls_s4_profile.sql) and is tracked as V4-1.

import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const migrationsDir = join(root, 'supabase', 'migrations');
const files = readdirSync(migrationsDir).filter((f) => f.endsWith('.sql') && f.includes('_s4p_'));
const sql = files.map((f) => readFileSync(join(migrationsDir, f), 'utf8')).join('\n');
const code = sql
  .split('\n')
  .filter((line) => !line.trim().startsWith('--'))
  .join('\n');

interface Matrix {
  tables: Record<string, Record<'select' | 'insert' | 'update' | 'delete', 'owner' | 'none'>>;
}
interface Contract {
  rpcs: { name: string; params: string[]; returns: string[]; roles: string[] }[];
  internal: { name: string; params: string[] }[];
}
const matrix = JSON.parse(readFileSync(join(root, 'fixtures', 'profile', 'rls-matrix.json'), 'utf8')) as Matrix;
const contract = JSON.parse(readFileSync(join(root, 'fixtures', 'profile', 'rpc-contract.json'), 'utf8')) as Contract;

const tables = [...code.matchAll(/create table public\.(\w+)\s*\(/g)].map((m) => m[1]);
const starts = [...code.matchAll(/create or replace function public\.(\w+)\(/g)];
const blocks = starts.map((m, i) => ({ name: m[1], text: code.slice(m.index, starts[i + 1]?.index ?? code.length) }));
const SECTIONS = ['room', 'grass', 'level', 'badges', 'guestbook', 'visitors', 'wiki', 'subject', 'gifts'];

describe('profile tables and row level security', () => {
  it('is exactly one migration', () => {
    expect(files).toHaveLength(1);
  });

  it('creates the tables of the matrix and only those', () => {
    expect([...tables].sort()).toEqual(Object.keys(matrix.tables).sort());
  });

  it('enables RLS everywhere and defines no policy and no grant on any table', () => {
    for (const table of tables) {
      expect(code, table).toMatch(new RegExp(`alter table public\\.${table}\\s+enable row level security`));
    }
    expect(code).not.toMatch(/create policy/);
    expect(code).not.toMatch(/grant (select|insert|update|delete|all)[^;]* on /);
    for (const ops of Object.values(matrix.tables)) expect(Object.values(ops).every((r) => r === 'none')).toBe(true);
    const revoke = code.match(/revoke all on ([^;]*) from public, anon, authenticated;/)?.[1] ?? '';
    for (const table of tables) expect(revoke, table).toContain(`public.${table}`);
  });

  it('defaults every section scope to private and constrains it to public/friends/private', () => {
    const body = code.match(/create table public\.profile_settings\s*\(([\s\S]*?)\n\);/)?.[1] ?? '';
    for (const section of SECTIONS) {
      expect(body, section).toMatch(
        new RegExp(`vis_${section}\\s+text not null default 'private' check \\(vis_${section} in \\('public', 'friends', 'private'\\)\\)`),
      );
    }
  });

  it('has no column for deviation statistics, titles, URLs or the character inputs', () => {
    const forbidden = /^(title|window_title|url|typed\w*|app_name|attempts\w*|deviation\w*|distraction\w*|mbti|birth\w*|gender)$/i;
    const bodies = [...code.matchAll(/create table public\.\w+\s*\(([\s\S]*?)\n\);/g)].map((m) => m[1]);
    expect(bodies.length).toBe(tables.length);
    for (const body of bodies) {
      for (const line of body.split('\n')) {
        const column = line.trim().match(/^(\w+)\s/)?.[1];
        if (column && !['unique', 'primary', 'check'].includes(column)) expect(column, line).not.toMatch(forbidden);
      }
    }
  });

  it('gift_catalog holds exactly the shop clothes and furniture of the app catalog', () => {
    const items = JSON.parse(readFileSync(join(root, 'src-tauri', 'assets', 'character', 'items.json'), 'utf8')) as {
      items: { id: string; kind: string; source: string }[];
    };
    const giftable = items.items.filter((i) => i.source === 'shop' && (i.kind === 'clothing' || i.kind === 'furniture')).map((i) => i.id);
    const inserted = [...(code.match(/insert into public\.gift_catalog \(item_id\) values([\s\S]*?);/)?.[1] ?? '').matchAll(/'(\w+)'/g)].map(
      (m) => m[1],
    );
    expect([...inserted].sort()).toEqual([...giftable].sort());
    for (const item of items.items.filter((i) => i.source === 'achievement' || i.source === 'starter' || i.kind === 'character')) {
      expect(inserted, item.id).not.toContain(item.id);
    }
  });
});

describe('profile security definer functions', () => {
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

  it('grants EXECUTE as the contract says, and anon only on get_public_profile', () => {
    for (const rpc of contract.rpcs) {
      const signature = `public\\.${rpc.name}\\([^)]*\\)`;
      const revoke = code.match(new RegExp(`revoke execute on function ${signature} from ([a-z_, ]+);`));
      const revoked = (revoke?.[1] ?? '').split(',').map((s) => s.trim());
      expect(revoked, `${rpc.name} revoke`).toContain('public');
      if (!rpc.roles.includes('anon')) expect(revoked, `${rpc.name} revoke anon`).toContain('anon');
      const grants = [...code.matchAll(new RegExp(`grant execute on function ${signature} to ([a-z_, ]+);`, 'g'))].flatMap((m) =>
        m[1].split(',').map((s) => s.trim()),
      );
      expect(grants.sort(), rpc.name).toEqual([...rpc.roles].sort());
    }
    const anonOpen = contract.rpcs.filter((r) => r.roles.includes('anon')).map((r) => r.name);
    expect(anonOpen).toEqual(['get_public_profile']);
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

  it('reports refusals as returned codes, never by RAISE', () => {
    for (const block of blocks) expect(block.text, block.name).not.toMatch(/\braise\b/i);
    for (const rpc of contract.rpcs) {
      const block = blocks.find((b) => b.name === rpc.name);
      const returned = [...(block?.text ?? '').matchAll(/'code', '(\w+)'/g)].map((m) => m[1]);
      for (const value of returned) expect(rpc.returns, `${rpc.name} returns ${value}`).toContain(value);
    }
  });

  it('identifies the caller with auth.uid(), never by a parameter', () => {
    for (const rpc of contract.rpcs) {
      const text = blocks.find((b) => b.name === rpc.name)?.text ?? '';
      expect(text, rpc.name).toContain('auth.uid()');
    }
  });

  it('answers an anonymous caller only through profile_view with a null viewer', () => {
    const text = blocks.find((b) => b.name === 'get_public_profile')?.text ?? '';
    expect(text).toContain('public.profile_view(v_owner, v_viewer)');
    // Nothing but a slug is taken; the viewer is whoever auth.uid() says (null when anonymous).
    expect(text).not.toMatch(/p_user|p_owner/);
    const scope = blocks.find((b) => b.name === 'profile_scope_allows')?.text ?? '';
    expect(scope).toMatch(/when p_scope = 'public' then true/);
    expect(scope).toMatch(/when p_scope = 'friends' then p_viewer is not null/);
    expect(scope).toMatch(/else false/);
  });

  it('applies the scope to every section of the profile view', () => {
    const view = blocks.find((b) => b.name === 'profile_view')?.text ?? '';
    for (const section of SECTIONS) {
      expect(view, section).toContain(`profile_scope_allows(s.vis_${section}, p_owner, p_viewer)`);
    }
  });

  it('limits gifts to catalog items, room mates, other people, a daily count and an idempotency key', () => {
    const gift = blocks.find((b) => b.name === 'send_gift')?.text ?? '';
    expect(gift).toContain('public.gift_catalog');
    expect(gift).toContain('public.profiles_share_room(v_user, p_recipient)');
    expect(gift).toContain('p_recipient = v_user');
    expect(gift).toMatch(/>= 5/);
    expect(gift).toContain('idem_key');
    expect(code).toMatch(/unique \(sender_id, idem_key\)/);
    // No points column exists anywhere, so points cannot be transferred.
    expect(code).not.toMatch(/\bpoints?\b\s+(integer|bigint)/i);
  });

  it('rate limits the guestbook per author and lets only a room mate write', () => {
    const text = blocks.find((b) => b.name === 'sign_guestbook')?.text ?? '';
    expect(text).toContain("interval '1 minute'");
    expect(text).toContain('public.profiles_share_room(v_user, p_owner)');
  });

  it('contains no service-role credential', () => {
    expect(sql).not.toMatch(/service_role_key|SERVICE_ROLE_KEY/);
  });
});
