import { Link } from "react-router";

import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Footer } from "@/components/ui/footer";

// 存在しない / 停止・削除済みの子域(`<sub>.<domain>` に service が無い)に来たときの着地点。
// traefik の catch-all router(route.rs::write_catchall)が apex の `/noservice` へ 302 する。
// 来訪者は平台ユーザとは限らない(=守衛の外・ログイン不要)。NotFound と同じ「中央カード +
// 島の樹冠」構図でブランドを保ちつつ、「ここにはアプリが無い → つぼみで自分の web app を作れる」
// と前向きに案内する。
export default function NoService() {
  return (
    <main className="relative flex min-h-dvh flex-col items-center justify-center overflow-hidden p-6">
      <PageMeta title="アプリがないみたい" />

      <Card className="w-full max-w-sm">
        <CardContent className="flex flex-col items-center gap-5 py-2 text-center">
          {/* ブランド(小):ロゴ + ワードマーク */}
          <div className="flex items-center gap-2">
            <img src="/logo.png" alt="" className="h-8 w-auto" />
            <span className="text-2xl font-extrabold tracking-tight text-card-foreground">
              つぼみ
            </span>
          </div>

          {/* 主役:芽のアイコン(「ここから育てよう」の含み)。 */}
          <div className="flex size-24 items-center justify-center rounded-full bg-foreground/5">
            <img
              src="/icons/icon-leaf.png"
              alt=""
              className="size-16 object-contain drop-shadow-sm"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <h2 className="text-lg font-bold text-card-foreground">
              ここにはまだ アプリがないみたい
            </h2>
            <p className="text-sm leading-relaxed text-card-foreground/80">
              この子ドメインには まだ何もデプロイされていないようです(停止または
              削除されたのかもしれません)。つぼみのトップから、自分の web アプリを
              作って公開しましょう。
            </p>
          </div>

          <Button asChild type="primary" size="middle">
            <Link to="/">つぼみで作る 🌱</Link>
          </Button>
        </CardContent>
      </Card>

      {/* 装飾:島の樹冠(全幅・下端。NotFound / Forbidden / ログイン画面と同じ) */}
      <Footer type="tree" className="pointer-events-none absolute inset-x-0 bottom-0" />
    </main>
  );
}
