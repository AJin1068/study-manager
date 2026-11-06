// Static checks of .github/workflows/android.yml. They read the workflow as text: they prove the
// structure (pinned actions, least privilege, no secret echo) and NOT that GitHub runs it.

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const text = readFileSync(join(root, '.github', 'workflows', 'android.yml'), 'utf8');
const lines = text.split('\n').filter((line) => !line.trim().startsWith('#'));

describe('android workflow', () => {
  it('pins every action to an explicit version', () => {
    const uses = lines.map((l) => l.match(/^\s*-?\s*uses:\s*(\S+)/)?.[1]).filter((u): u is string => !!u);
    expect(uses.length).toBeGreaterThanOrEqual(4);
    for (const ref of uses) expect(ref).toMatch(/^[\w.-]+\/[\w.-]+@(v\d+(\.\d+){0,2}|[0-9a-f]{40})$/);
  });

  it('does not run on pull requests and starts with read-only permissions', () => {
    expect(text).not.toMatch(/pull_request/);
    expect(text).toMatch(/^permissions:\n\s+contents: read$/m);
  });

  it('reads secrets only through env and never prints them', () => {
    const secretLines = lines.filter((l) => l.includes('secrets.'));
    expect(secretLines.length).toBeGreaterThan(0);
    for (const line of secretLines) expect(line).toMatch(/^\s+[A-Z0-9_]+: \$\{\{ secrets\.[A-Z0-9_]+ \}\}$/);
    for (const line of lines) {
      if (/\b(echo|printf)\b/.test(line)) expect(line).not.toMatch(/ANDROID_KEYSTORE_PASSWORD|secrets\./);
      expect(line).not.toMatch(/set -x|--debug|::add-mask/);
    }
    expect(text).not.toMatch(/run:.*\$\{\{\s*secrets\./);
  });

  it('removes the decoded keystore and uploads the signed APK', () => {
    expect(text).toContain('rm -f "$RUNNER_TEMP/release.jks"');
    expect(text).toContain('actions/upload-artifact@');
    expect(text).toContain('if-no-files-found: error');
  });
});
