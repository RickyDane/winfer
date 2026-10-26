import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import process from "node:process";
import path from "node:path";
import type { Plugin } from "vite";

/**
 * Dev only: `/__mock/favicon?url=…` fetches a page icon server-side, like the Tauri backend does,
 * so the mocked browser (src/dev/mockTauri.ts) shows real icons the panel can read the pixels of.
 */
function mockFavicons(): Plugin {
  return {
    name: "mock-favicons",
    apply: "serve",
    configureServer(server) {
      server.middlewares.use("/__mock/favicon", async (req, res) => {
        const url = new URL(req.url ?? "", "http://localhost").searchParams.get("url") ?? "";
        try {
          const icon = await fetch(url, { headers: { "User-Agent": "winfer" }, signal: AbortSignal.timeout(10_000) });
          const type = icon.headers.get("content-type") ?? "";
          if (!icon.ok || !(type.startsWith("image/") || url.endsWith(".ico"))) throw new Error(`no icon (${icon.status} ${type})`);
          res.setHeader("Content-Type", type.startsWith("image/") ? type : "image/x-icon");
          res.end(Buffer.from(await icon.arrayBuffer()));
        } catch (err) {
          res.statusCode = 404;
          res.end(String(err));
        }
      });
    },
  };
}

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react(), tailwindcss(), mockFavicons()],
  test: {
    // Harness Bench task fixtures hold deliberately broken repos for agents to fix; not our tests.
    // mobile/ and relay/ are separate packages with their own vitest setups.
    // The standalone mockup uses node:test, run with node --test instead of Vitest.
    exclude: ["**/node_modules/**", "**/dist/**", "plugins-src/harness-bench/**", "mobile/**", "relay/**", "mockups/tui-refinements/cosmos.test.mjs"],
  },
  resolve: {
    alias: { "@": path.resolve(import.meta.dirname, "src") },
  },
  // Only the app's own page: mobile/index.html (the PWA) has its own React, and pre-bundling
  // both copies breaks hooks in the desktop app.
  optimizeDeps: { entries: ["index.html"] },
  // The syntax-highlighting worker loads language grammars on demand.
  worker: { format: "es" },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      // and the bench's task fixtures (some are HTML sites, which would reload the page)
      ignored: ["**/src-tauri/**", "**/plugins-src/**"],
    },
  },
}));
