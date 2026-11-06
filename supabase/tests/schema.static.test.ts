// Static checks of the server SQL. They read the migration as text, so they prove structure
// (RLS switched on, definer functions pinned and revoked, policies matching the shared matrix,
// no secrets) and NOT that Postgres accepts or enforces it: that needs `supabase test db`
// (tests/rls_s2.sql) and is tracked as V2-8.

import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const migrationsDir = join(root, 'supabase', 'migrations');
const migrations = readdirSync(migrationsDir).filter((f) => f.endsWith('.sql')).sort();
const sql = migrations.map((f) => readFileSync(join(migrationsDir, f), 'utf8')).join('\n');
// Comment lines are documentation (the cron example lives there); policy checks look at code only.
const code = sql
  .split('\n')
  .filter((line) => !line.trim().startsWith('--'))
  .join('\n');

interface Matrix {
  tables: Record<string, Record<'select' | 'insert' | 'update' | 'delete', 'owner' | 'none'>>;
}
interface Contract {
  rpcs: { name: string; params: string[]; returns: string[]; roles: string[] }[];
}
const matrix = JSON.parse(readFileSync(join(root, 'fixtures', 'sync', 'rls-matrix.json'), 'utf8')) as Matrix;
const contract = JSON.parse(readFileSync(join(root, 'fixtures', 'sync', 'rpc-contract.json'), 'utf8')) as Contract;

const tables = [...code.matchAll(/create table public\.(\w+)\s*\(/g)].map((m) => m[1]);

function functionBlocks(): { name: string; text: string }[] {
  const starts = [...code.matchAll(/create or replace function public\.(\w+)\(/g)];
  return starts.map((m, i) => ({
    name: m[1],
    text: code.slice(m.index, starts[i + 1]?.index ?? code.length),
  }));
}

function walk(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    if (entry === 'node_modules' || entry === 'target' || entry === 'dist' || entry.startsWith('.')) continue;
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) walk(full, out);
    else out.push(full);
  }
  return out;
}

describe('row level security', () => {
  it('creates the tables the shared matrix describes, and only those', () => {
    expect([...tables].sort()).toEqual(Object.keys(matrix.tables).sort());
  });

  it('enables RLS on every public table', () => {
    for (const table of tables) {
      expect(code, table).toMatch(new RegExp(`alter table public\\.${table}\\s+enable row level security`));
    }
  });

  it('has a policy for exactly the operations the matrix marks as owner', () => {
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

  it('revokes all table privileges from anon and public, and grants only what the matrix allows', () => {
    expect(code).toMatch(/revoke all on [^;]*from public, anon, authenticated;/);
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

describe('security definer functions', () => {
  const blocks = functionBlocks();

  it('covers exactly the RPCs in the shared contract', () => {
    expect(blocks.map((b) => b.name).sort()).toEqual(contract.rpcs.map((r) => r.name).sort());
  });

  it('pins search_path on every definer function', () => {
    for (const block of blocks) {
      expect(block.text, block.name).toContain('security definer');
      expect(block.text, block.name).toMatch(/set search_path = public, pg_temp/);
    }
  });

  it('revokes EXECUTE from public and anon, then grants only the contract roles', () => {
    for (const rpc of contract.rpcs) {
      const signature = `public\\.${rpc.name}\\([^)]*\\)`;
      const revoke = code.match(new RegExp(`revoke execute on function ${signature} from ([a-z_, ]+);`));
      expect(revoke, `${rpc.name} revoke`).not.toBeNull();
      const revoked = (revoke?.[1] ?? '').split(',').map((s) => s.trim());
      expect(revoked, rpc.name).toEqual(expect.arrayContaining(['public', 'anon']));
      const grants = [...code.matchAll(new RegExp(`grant execute on function ${signature} to ([a-z_, ]+);`, 'g'))].flatMap(
        (m) => m[1].split(',').map((s) => s.trim()),
      );
      expect(grants.sort(), rpc.name).toEqual([...rpc.roles].sort());
    }
  });

  it('keeps claim_alert away from authenticated users', () => {
    const claim = contract.rpcs.find((r) => r.name === 'claim_alert');
    expect(claim?.roles).toEqual(['service_role']);
    expect(code).toMatch(/revoke execute on function public\.claim_alert\([^)]*\) from [^;]*authenticated/);
  });

  it('declares the contract parameters in order', () => {
    for (const rpc of contract.rpcs) {
      const block = blocks.find((b) => b.name === rpc.name);
      const header = block?.text.slice(0, block.text.indexOf('returns')) ?? '';
      const names = [...header.matchAll(/\b(p_\w+)\s/g)].map((m) => m[1]);
      expect(names, rpc.name).toEqual(rpc.params);
    }
  });

  it('reports failures as returned codes, never by RAISE (a rollback would erase the attempt record)', () => {
    for (const block of blocks) {
      expect(block.text, block.name).not.toMatch(/\braise\b/i);
      const returned = [...block.text.matchAll(/return '(\w+)'/g)].map((m) => m[1]);
      const contractCodes = contract.rpcs.find((r) => r.name === block.name)?.returns ?? [];
      for (const value of returned) expect(contractCodes, `${block.name} returns ${value}`).toContain(value);
    }
  });

  it('checks ownership inside user-facing RPCs with auth.uid()', () => {
    for (const block of blocks.filter((b) => b.name !== 'claim_alert')) {
      expect(block.text, block.name).toContain('auth.uid()');
      expect(block.text, block.name).toContain('d.user_id = v_user');
    }
  });
});

describe('what the schema may hold', () => {
  it('has no column that could store window titles, URLs, or typed text', () => {
    const forbidden = /^(title|window_title|url|typed\w*|sentence\w*)$/i;
    const bodies = [...code.matchAll(/create table public\.\w+\s*\(([\s\S]*?)\n\);/g)].map((m) => m[1]);
    expect(bodies.length).toBe(tables.length);
    for (const body of bodies) {
      for (const line of body.split('\n')) {
        const column = line.trim().match(/^(\w+)\s/)?.[1];
        if (column) expect(column, line).not.toMatch(forbidden);
      }
    }
  });

  it('every user table has a user_id', () => {
    for (const table of tables) {
      const body = code.match(new RegExp(`create table public\\.${table}\\s*\\(([\\s\\S]*?)\\n\\);`))?.[1] ?? '';
      expect(body, table).toMatch(/\buser_id\b/);
    }
  });
});

describe('no secrets in the repository', () => {
  const secretPatterns: [string, RegExp][] = [
    ['JWT', /eyJ[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{8,}/],
    ['service role key variable', /SERVICE_ROLE_KEY|service_role_key/i],
    ['provider key', /\bsk-[a-z]{2,}-[A-Za-z0-9]{10,}/],
  ];

  it('SQL, app code, and fixtures carry no credential-shaped text', () => {
    const roots = ['supabase', 'src', join('src-tauri', 'src'), join('src-tauri', 'tests'), 'fixtures'].map((r) => join(root, r));
    const files = roots.filter(existsSync).flatMap((r) => walk(r));
    expect(files.length).toBeGreaterThan(20);
    for (const file of files) {
      if (file.endsWith('schema.static.test.ts')) continue;
      if (!/\.(sql|ts|tsx|rs|json|md)$/.test(file)) continue;
      const text = readFileSync(file, 'utf8');
      for (const [label, pattern] of secretPatterns) {
        expect(text, `${label} in ${relative(root, file)}`).not.toMatch(pattern);
      }
    }
  });

  it('the cron example reads its credential from Vault and embeds none', () => {
    const cron = sql.split('\n').filter((l) => l.trim().startsWith('--') && /net\.http_post|Authorization|Bearer|decrypted_secret/.test(l));
    expect(cron.some((l) => l.includes('vault.decrypted_secrets'))).toBe(true);
    for (const line of cron) expect(line).not.toMatch(/Bearer\s+['"]?[A-Za-z0-9._-]{12,}/);
    // Nothing is scheduled by a plain migration run.
    expect(code).not.toMatch(/cron\.schedule/);
  });
});

describe('pgTAP file', () => {
  it('declares a plan equal to the number of assertions', () => {
    const text = readFileSync(join(root, 'supabase', 'tests', 'rls_s2.sql'), 'utf8');
    const planned = Number(text.match(/select plan\((\d+)\)/)?.[1]);
    const assertions = [...text.matchAll(/^select (is|isnt|ok|throws_ok|lives_ok)\(/gm)].length;
    expect(assertions).toBe(planned);
  });
});
