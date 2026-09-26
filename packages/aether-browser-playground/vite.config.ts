import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
  },
  // Pre-bundling would rewrite the `new URL("browser_bg.wasm", import.meta.url)` the wasm-pack glue loads its module from.
  optimizeDeps: { exclude: ["@aether-agent/browser"] },
});
