// Static checks of the phone_app notification channel (S5.7): the SQL constraint, the Edge Function
// treatment and the shared contract. They read text; they do not prove Postgres enforces it
// (tests/rls_s5_notify_phone_app.sql does, and has not been run: V5-8).

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const read = (...parts: string[]) => readFileSync(join(root, ...parts), 'utf8');
const sql = read('supabase', 'migrations', '20260601000600_s5_notify_phone_app.sql')
  .split('\n')
  .filter((line) => !line.trim().startsWith('--'))
  .join('\n');
const core = read('supabase', 'migrations', '20260601000000_s2_core.sql');
const contract = JSON.parse(read('fixtures', 'notify', 'contract.json')) as {
  messages: { test: string };
  phoneApp: {
    channelRow: Record<string, unknown>;
    bodies: { generic: string; fixed: string[]; replacedByGeneric: string[] };
  };
};

describe('phone_app channel', () => {
  it('is an allowed channel value and requires an ntfy topic', () => {
    expect(core).toMatch(/channel in \('none', 'ntfy', 'telegram', 'phone_app'\)/);
    expect(sql).toMatch(/add constraint notify_channels_phone_app_needs_topic/);
    expect(sql).toMatch(/check \(channel <> 'phone_app' or ntfy_topic is not null\)/);
  });

  it('adds no policy or grant', () => {
    expect(sql).not.toMatch(/create policy|grant |revoke |drop /i);
  });

  it('is sent by the Edge Function through the ntfy path', () => {
    const fn = read('supabase', 'functions', 'missed-plan', 'index.ts');
    expect(fn).toMatch(/row\.channel === 'ntfy' \|\| row\.channel === 'phone_app'\) && row\.ntfy_topic/);
  });

  it('has a contract row that satisfies the constraint and bodies without a subject', () => {
    const row = contract.phoneApp.channelRow;
    expect(row.channel).toBe('phone_app');
    expect(typeof row.ntfy_topic).toBe('string');
    expect(contract.phoneApp.bodies.fixed).toContain(contract.messages.test);
    for (const text of [...contract.phoneApp.bodies.fixed, contract.phoneApp.bodies.generic]) {
      expect(text).not.toMatch(/수학|영어|과목|math|subject/i);
    }
  });
});
