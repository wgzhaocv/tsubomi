import { PageMeta } from "@/components/page-meta";
import { Card, CardContent } from "@/components/ui/card";
import { Footer } from "@/components/ui/footer";

// 社内ドメイン外のアカウントで Google ログインを試みたときに飛ばされる専用画面。
// サーバ側のドメイン検証(auth/google.rs)で弾かれた直後にここへリダイレクトされる。
// 終端ページ:同じブラウザ(=1 プロファイル 1 アカウント)で入り直すだけなので
// 行動ボタンは置かず、状況だけ 1 枚のカードに収めて画面中央に置く。

export default function Forbidden() {
  return (
    <main className="relative flex min-h-dvh flex-col items-center justify-center overflow-hidden p-6">
      <PageMeta title="権限がありません" />

      <Card className="w-full max-w-sm">
        <CardContent className="flex flex-col items-center gap-5 py-2 text-center">
          {/* ブランド(小):ロゴ + ワードマーク */}
          <div className="flex items-center gap-2">
            <img src="/logo.png" alt="" className="h-8 w-auto" />
            <span className="text-2xl font-extrabold tracking-tight text-card-foreground">
              つぼみ
            </span>
          </div>

          {/* 鍵アイコン(AC 風・自作)。Retina 用に 1x/2x/3x を srcset で出す。 */}
          <div className="flex size-24 items-center justify-center rounded-full bg-foreground/5">
            <img
              src="/icons/icon-lock.png"
              srcSet="/icons/icon-lock.png 1x, /icons/icon-lock@2x.png 2x, /icons/icon-lock@3x.png 3x"
              alt=""
              className="size-16 object-contain"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <h2 className="text-lg font-bold text-card-foreground">利用権限がありません</h2>
            <p className="text-sm text-card-foreground/80">
              会社の Google アカウントでログインしてください。
            </p>
          </div>
        </CardContent>
      </Card>

      {/* 装飾:島の樹冠(全幅・下端。ログイン画面と同じ) */}
      <Footer type="tree" className="pointer-events-none absolute inset-x-0 bottom-0" />
    </main>
  );
}
