import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// `pnpm tauri dev` reads `devUrl` from tauri.conf.json (port 1420).
// Vite picks that up via the TAURI_DEV_HOST env var when run from the
// CLI; we hard-pin the port so non-Tauri `pnpm dev` matches.
export default defineConfig(async () => ({
  plugins: [react()],
  // Prevent Vite from clearing the screen so cargo's tauri output
  // stays visible.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: process.env.TAURI_DEV_HOST || false,
    hmr: process.env.TAURI_DEV_HOST
      ? {
          protocol: "ws",
          host: process.env.TAURI_DEV_HOST,
          port: 1421,
        }
      : undefined,
    watch: {
      // tauri-build regenerates `src-tauri/gen/...` on every dev run;
      // ignore it so Vite doesn't bounce.
      ignored: ["**/src-tauri/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: !process.env.TAURI_ENV_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
}));
