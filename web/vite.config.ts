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
    // API 呼び出しを axum サーバへ転送し、dev でも SPA を same-origin に保つ。
    // 9090:8080 は amber が使う(衝突回避)。
    proxy: {
      "/api": "http://localhost:9090",
    },
  },
});
