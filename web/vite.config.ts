import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite-plus";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

// https://vite.dev/config/
export default defineConfig({
  fmt: {},
  // no-floating-promises は無効化。fire-and-forget の navigate/refetch/clipboard を
  // `void` で抑止するのを止め、コードから void 演算子を一掃する方針(プロジェクト決定)。
  lint: {
    options: { typeAware: true, typeCheck: true },
    rules: { "typescript/no-floating-promises": "off" },
  },
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  server: {
    // API 呼び出しを axum サーバへ転送し、dev でも SPA を same-origin に保つ。
    // 9090:8080 は amber が使う(衝突回避)。ws:true = リソース概要の host 指標 WS
    // (/api/admin/metrics)も dev で転送する。
    proxy: {
      "/api": { target: "http://localhost:9090", ws: true },
    },
  },
});
