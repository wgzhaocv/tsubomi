import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { RouterProvider } from "react-router";
import { QueryClientProvider } from "@tanstack/react-query";

// 自托管字体(Nunito=ラテン / Noto Sans JP=日本語)。woff2 はビルドに同梱され、
// unicode-range でサブセット化されて必要分だけ読み込む(実行時の外網依存なし)。
import "@fontsource-variable/nunito/index.css";
import "@fontsource-variable/noto-sans-jp/index.css";
import "./index.css";
import { queryClient } from "@/lib/query-client";
import { router } from "@/router";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {/* サーバ状態は QueryClient が一元管理する(認証など)。 */}
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
);
