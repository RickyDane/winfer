import fs from 'node:fs';
import path from 'node:path';

/** Minimal dotenv parser: KEY=value lines, optional `export `, optional quotes, # comment lines. */
export function parseEnv(text) {
  const out = {};
  for (const raw of text.split('\n')) {
    const line = raw.trim();
    if (!line || line.startsWith('#')) continue;
    const m = /^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$/.exec(line);
    if (!m) continue;
    let value = m[2];
    if (/^(['"]).*\1$/.test(value)) value = value.slice(1, -1);
    out[m[1]] = value;
  }
  return out;
}

/**
 * Loads env files for `mode`. Later files override earlier ones.
 * `.env.local` is skipped for production builds so local hacks never ship.
 */
export function loadEnvFiles(mode, cwd) {
  const files = ['.env', `.env.${mode}`, '.env.local', `.env.${mode}.local`];
  const vars = {};
  for (const name of files) {
    if (name === '.env.local' && mode === 'production') continue;
    const file = path.join(cwd, name);
    if (fs.existsSync(file)) Object.assign(vars, parseEnv(fs.readFileSync(file, 'utf8')));
  }
  return vars;
}
