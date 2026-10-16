import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed port in dev and serves the built files in release. On a phone the dev
// server is reached over the network (`TAURI_DEV_HOST`, set by `tauri android|ios dev`).
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || "127.0.0.1",
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
  },
  build: { target: "es2022", outDir: "dist", emptyOutDir: true },
});
