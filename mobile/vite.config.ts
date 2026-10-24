import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

const src = fileURLToPath(new URL("../src", import.meta.url));

// Served at the relay Worker's origin root; relative base keeps the build portable.
export default defineConfig({
  base: "./",
  plugins: [react()],
  resolve: {
    // winfer code's web UI (../src/features/code) is the desktop's own; `@` is the desktop's src.
    alias: { "@": src },
    // Its imports resolve to this app's copies, so there's one React (and one zustand).
    dedupe: ["react", "react-dom", "zustand", "clsx", "lucide-react", "react-markdown", "remark-gfm"],
  },
  server: { host: true, port: 5180, fs: { allow: [".", src] } },
  build: { outDir: "dist", emptyOutDir: true },
  test: { environment: "node", include: ["src/**/*.test.ts"] },
});
