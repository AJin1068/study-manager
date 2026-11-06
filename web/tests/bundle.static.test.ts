import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

describe('bundle contents', () => {
  const root = join(dirname(fileURLToPath(import.meta.url)), '..');
  const walk = (dir: string): string[] =>
    readdirSync(dir).flatMap((name: string) => {
      const path = join(dir, name);
      return statSync(path).isDirectory() ? walk(path) : [path];
    });
  const sources = [...walk(join(root, 'src')), ...walk(join(root, 'public'))].filter((f) => !f.endsWith('.test.tsx'));

  it('has no service-role credential and no deviation or title field', () => {
    expect(sources.length).toBeGreaterThan(5);
    for (const file of sources) {
      const text = readFileSync(file, 'utf8').toLowerCase();
      expect(text, file).not.toContain('service_role');
      expect(text, file).not.toMatch(/sb_secret|eyj[a-z0-9_-]{20,}/);
      expect(text, file).not.toMatch(/window_title|app_name|attempts_by_hour|attempts_today/);
    }
  });

  it('ships config.json with no key and the built bundle (when built) has none either', () => {
    const config = JSON.parse(readFileSync(join(root, 'public', 'config.json'), 'utf8')) as { anonKey: string };
    expect(config.anonKey).toBe('');
    const dist = join(root, 'dist');
    if (!existsSync(dist)) return;
    for (const file of walk(dist)) {
      if (!/\.(js|html|json|css)$/.test(file)) continue;
      expect(readFileSync(file, 'utf8').toLowerCase(), file).not.toContain('service_role');
    }
  });
});
