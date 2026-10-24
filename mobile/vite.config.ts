import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Served at the relay Worker's origin root; relative base keeps the build portable.
export default defineConfig({
  base: "./",
  plugins: [react()],
  // ../src/features/providers/logos.ts is shared with the desktop app.
  server: { host: true, port: 5180, fs: { allow: [".", "../src/features/providers"] } },
  build: { outDir: "dist", emptyOutDir: true },
  test: { environment: "node", include: ["src/**/*.test.ts"] },
});
