import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { RouterProvider } from "react-router";

// 自托管字体(Nunito=ラテン / Noto Sans JP=日本語)。woff2 はビルドに同梱され、
// unicode-range でサブセット化されて必要分だけ読み込む(実行時の外網依存なし)。
import "@fontsource-variable/nunito/index.css";
import "@fontsource-variable/noto-sans-jp/index.css";
import "./index.css";
import { router } from "@/router";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <RouterProvider router={router} />
  </StrictMode>,
);
