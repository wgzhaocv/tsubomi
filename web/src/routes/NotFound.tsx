import { Link } from "react-router";

import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Footer } from "@/components/ui/footer";

// 未知のパス(router の catch-all `*`)に出す 404。Forbidden と同じ「中央のカード + 島の
// 樹冠」構図で、ブランドを崩さず迷子をやさしく案内する。葉っぱを「道に迷って落ちてきた」
// 風に添える(design-reference の AC 風 ── 暖色・ミント・丸み)。
export default function NotFound() {
  return (
    <main className="relative flex min-h-dvh flex-col items-center justify-center overflow-hidden p-6">
      <PageMeta title="ページが見つかりません" />

      <Card className="w-full max-w-sm">
        <CardContent className="flex flex-col items-center gap-5 py-2 text-center">
          {/* ブランド(小):ロゴ + ワードマーク */}
          <div className="flex items-center gap-2">
            <img src="/logo.png" alt="" className="h-8 w-auto" />
            <span className="text-2xl font-extrabold tracking-tight text-card-foreground">
              つぼみ
            </span>
          </div>

          {/* 404 の主役。落ち葉を添えて「道に迷った」雰囲気に。 */}
          <div className="flex flex-col items-center gap-1">
            <img
              src="/icons/icon-leaf.png"
              alt=""
              className="size-11 -rotate-12 object-contain drop-shadow-sm"
            />
            <span className="text-[64px] leading-none font-black tracking-tighter text-[#0CC0B5] [text-shadow:0_3px_0_rgba(10,158,149,0.25)]">
              404
            </span>
          </div>

          <div className="flex flex-col gap-1.5">
            <h2 className="text-lg font-bold text-card-foreground">ページが見つかりません</h2>
            <p className="text-sm leading-relaxed text-card-foreground/80">
              お探しのページは見つかりませんでした。アドレスが変わったか、削除された
              可能性があります。
            </p>
          </div>

          <Button asChild type="primary" size="middle">
            <Link to="/">はじめに戻る 🌷</Link>
          </Button>
        </CardContent>
      </Card>

      {/* 装飾:島の樹冠(全幅・下端。Forbidden / ログイン画面と同じ) */}
      <Footer type="tree" className="pointer-events-none absolute inset-x-0 bottom-0" />
    </main>
  );
}
