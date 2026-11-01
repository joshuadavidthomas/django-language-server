import { defineConfig } from "vite";

export default defineConfig({
  // The djls worker and the wasm-bindgen glue are ES modules.
  worker: { format: "es" },
  // Monaco alone is several megabytes; splitting it buys nothing here.
  build: { chunkSizeWarningLimit: 5000 },
  // Prebundling splits Monaco's side-effect imports into separate copies, so
  // contributions such as Go to Definition miss the editor's registry.
  optimizeDeps: { exclude: ["monaco-editor"] },
  server: { allowedHosts: true },
});
