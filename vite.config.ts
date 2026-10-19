import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import process from "node:process";
import path from "node:path";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react(), tailwindcss()],
  test: {
    // Harness Bench task fixtures hold deliberately broken repos for agents to fix; not our tests.
    // mobile/ and relay/ are separate packages with their own vitest setups.
    exclude: ["**/node_modules/**", "**/dist/**", "plugins-src/harness-bench/**", "mobile/**", "relay/**"],
  },
  resolve: {
    alias: { "@": path.resolve(import.meta.dirname, "src") },
  },
  // Only the app's own page: mobile/index.html (the PWA) has its own React, and pre-bundling
  // both copies breaks hooks in the desktop app.
  optimizeDeps: { entries: ["index.html"] },

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
