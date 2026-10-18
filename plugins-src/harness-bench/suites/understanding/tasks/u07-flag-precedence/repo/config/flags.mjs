import fs from 'node:fs';
import path from 'node:path';
import { coerce, envName } from './coerce.mjs';
import { loadEnvFiles } from './env.mjs';

const read = (cwd, name) => JSON.parse(fs.readFileSync(path.join(cwd, 'config', name), 'utf8'));

/**
 * Resolves feature flags for a build.
 * Precedence (lowest -> highest): defaults, env files, process env, per-mode overrides, kill switches.
 */
export function resolveFlags({ mode, env, cwd }) {
  const defaults = read(cwd, 'flags.defaults.json');
  const overrides = read(cwd, 'flags.overrides.json');

  // Process env always wins over env files.
  const vars = Object.assign({}, env, loadEnvFiles(mode, cwd));

  const flags = { ...defaults };
  for (const [key, fallback] of Object.entries(defaults)) {
    const name = envName(key);
    if (vars[name] !== undefined && vars[name] !== '') flags[key] = coerce(vars[name], fallback);
  }

  const modeName = env.NODE_ENV === 'production' ? 'production' : mode;
  for (const [key, value] of Object.entries(overrides[modeName] ?? {})) {
    if (!(key in env)) flags[key] = value;
  }

  for (const key of (vars.KILL_SWITCHES ?? '').split(',').map((s) => s.trim()).filter(Boolean)) {
    flags[key] = false;
  }
  return flags;
}
