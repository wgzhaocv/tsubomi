import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

// 管理画面の各ページの内容コンテナ。横幅と既定パディングをここで一元管理する。
// 以前は DashboardLayout が pathname の正規表現で幅を出し分けていたが、それは層が
// 浅い(レイアウトがルートを知ってしまう)ので、各ページが自分の幅を宣言する形へ。
//   wide(既定): max-w-360 — 全ダッシュボード画面を統一(一覧・詳細・テーブルすべて)。
//   default:    max-w-5xl — 狭くしたい画面が出たとき明示する用に残す(現状不使用)。
export function PageContainer({
  width = "wide",
  className,
  children,
}: {
  width?: "default" | "wide";
  className?: string;
  children: ReactNode;
}) {
  return (
    <div
      className={cn(
        "mx-auto w-full p-6 md:p-10",
        width === "wide" ? "max-w-360" : "max-w-5xl",
        className,
      )}
    >
      {children}
    </div>
  );
}
