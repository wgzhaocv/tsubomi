import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite-plus";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

// https://vite.dev/config/
export default defineConfig({
  fmt: {},
  lint: { options: { typeAware: true, typeCheck: true } },
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  server: {
    // Forward API calls to the axum server so the SPA stays same-origin in dev.
    proxy: {
      "/api": "http://localhost:8080",
    },
  },
});
