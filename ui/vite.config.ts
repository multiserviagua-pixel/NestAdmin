import { defineConfig } from "vite";

// Puerto fijo: tauri.conf.json apunta a él en desarrollo.
export default defineConfig({
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { target: "es2022", outDir: "dist", emptyOutDir: true },
});
