import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import { resolveFlags } from '../config/flags.mjs';

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), '..');
const mode = process.env.APP_MODE ?? 'development';
const flags = resolveFlags({ mode, env: process.env, cwd: root });

await build({
  entryPoints: [path.join(root, 'src/main.ts')],
  bundle: true,
  minify: mode !== 'development',
  outdir: path.join(root, 'dist'),
  define: { __FLAGS__: JSON.stringify(flags) },
});
console.log(`built ${mode}`, flags);
