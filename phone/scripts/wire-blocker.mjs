// Connects the Kotlin blocker plugin (phone/plugins/blocker/android) to the Android project that
// `tauri android init` generates under phone/src-tauri/gen/android. gen/ is not committed, so this
// runs after every init. Idempotent. Usage: node phone/scripts/wire-blocker.mjs
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const gen = join(here, '..', 'src-tauri', 'gen', 'android');
const plugin = join(here, '..', 'plugins', 'blocker', 'android');
const settings = join(gen, 'settings.gradle');
const appGradle = join(gen, 'app', 'build.gradle.kts');

if (!existsSync(settings) || !existsSync(appGradle)) {
  console.error('gen/android is missing: run `tauri android init --ci` in phone/src-tauri first');
  process.exit(1);
}

const rel = relative(gen, plugin).split('\\').join('/');
const includeLine = `include ':blocker'\nproject(':blocker').projectDir = file('${rel}')\n`;
const depLine = '    implementation(project(":blocker"))\n';

let s = readFileSync(settings, 'utf8');
if (!s.includes("include ':blocker'")) {
  s += (s.endsWith('\n') ? '' : '\n') + includeLine;
  writeFileSync(settings, s);
}

let a = readFileSync(appGradle, 'utf8');
if (!a.includes('project(":blocker")')) {
  // Tolerates CRLF from the generator; an unknown template must fail loudly, never ship without the plugin.
  a = a.replace(/dependencies \{\r?\n/, (m) => m + depLine);
  writeFileSync(appGradle, a);
}
const wired =
  readFileSync(settings, 'utf8').includes("include ':blocker'") &&
  readFileSync(appGradle, 'utf8').includes('project(":blocker")');
if (!wired) {
  console.error('could not wire the blocker plugin: unexpected gen/android template');
  process.exit(1);
}
console.log('blocker plugin wired into', gen);
