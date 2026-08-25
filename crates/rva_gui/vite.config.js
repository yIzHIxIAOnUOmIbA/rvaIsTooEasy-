import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";
import { readFileSync } from "node:fs";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// Inject a unique version number at build time: package version + build timestamp (to the minute).
// __APP_VERSION__ changes on every build, so the HUD bottom-left instantly shows which build is running,
// preventing hard-to-notice issues like "source changed but the exe still embeds the old frontend".
const pkg = JSON.parse(readFileSync(new URL("./package.json", import.meta.url), "utf-8"));
const now = new Date();
const pad = (n) => String(n).padStart(2, "0");
const stamp =
  `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}` +
  `-${pad(now.getHours())}${pad(now.getMinutes())}`;
const APP_VERSION = `${pkg.version}-b${stamp}`;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [sveltekit()],
  define: {
    __APP_VERSION__: JSON.stringify(APP_VERSION),
  },

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
      ignored: ["**/src-tauri/**"],
    },
  },
}));
