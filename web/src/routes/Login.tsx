import { Navigate } from "react-router";

import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Footer } from "@/components/ui/footer";
import { Typewriter } from "@/components/ui/typewriter";
import { useMeQuery } from "@/lib/auth";

// ログイン画面。Google ログインのみ(社内アカウントに hd ドメイン制限あり)。
// 認証状態は useMeQuery で読む:既にログイン済みなら管理画面(/)へ送る。

// Google ブランドの「G」マーク(公式 4 色)。暖色 UI に対しても識別性のため原色のまま。
function GoogleMark() {
  return (
    <svg width="18" height="18" viewBox="0 0 18 18" aria-hidden="true">
      <path
        fill="#4285F4"
        d="M17.64 9.2c0-.64-.06-1.25-.16-1.84H9v3.48h4.84a4.14 4.14 0 0 1-1.8 2.72v2.26h2.92c1.7-1.57 2.68-3.88 2.68-6.62Z"
      />
      <path
        fill="#34A853"
        d="M9 18c2.43 0 4.47-.8 5.96-2.18l-2.92-2.26c-.81.54-1.84.86-3.04.86-2.34 0-4.32-1.58-5.03-3.7H.96v2.33A9 9 0 0 0 9 18Z"
      />
      <path
        fill="#FBBC05"
        d="M3.97 10.72A5.4 5.4 0 0 1 3.68 9c0-.6.1-1.18.29-1.72V4.95H.96A9 9 0 0 0 0 9c0 1.45.35 2.83.96 4.05l3.01-2.33Z"
      />
      <path
        fill="#EA4335"
        d="M9 3.58c1.32 0 2.5.45 3.44 1.35l2.58-2.58C13.46.89 11.43 0 9 0A9 9 0 0 0 .96 4.95l3.01 2.33C4.68 5.16 6.66 3.58 9 3.58Z"
      />
    </svg>
  );
}

export default function Login() {
  const { data: me, isPending, error } = useMeQuery();

  // ログイン済みなら管理画面へ。
  if (me) return <Navigate to="/" replace />;

  return (
    <main className="relative flex min-h-dvh flex-col items-center justify-center gap-8 overflow-hidden p-6">
      <PageMeta title="ログイン" />

      {/* ヒーロー:ロゴ + ワードマーク + キャッチ */}
      <div className="flex flex-col items-center gap-3">
        <img
          src="/logo.png"
          alt=""
          className="h-20 w-auto drop-shadow-[0_4px_8px_rgba(61,52,40,0.12)]"
        />
        <h1 className="text-5xl font-extrabold tracking-tight text-foreground">つぼみ</h1>
        <p className="text-sm font-bold text-foreground/70">社内 PaaS プラットフォーム</p>
      </div>

      {/* ログインカード */}
      <Card className="w-full max-w-sm">
        <CardContent className="flex flex-col items-center gap-5 py-2">
          <p className="min-h-10 text-center text-sm font-medium text-card-foreground">
            <Typewriter speed={40}>
              おかえりなさい。会社の Google アカウントでログインしてください。
            </Typewriter>
          </p>

          {error && (
            <p className="w-full text-center text-sm wrap-break-word text-destructive">
              {String(error)}
            </p>
          )}

          <Button
            asChild
            type="primary"
            size="large"
            block
            disabled={isPending}
            icon={<GoogleMark />}
          >
            <a href="/api/auth/google/start">Google でログイン</a>
          </Button>

          <p className="text-center text-xs text-muted-foreground">
            許可されたドメインの会社アカウントのみログインできます。
          </p>
        </CardContent>
      </Card>

      {/* 装飾:島の樹冠(全幅・下端) */}
      <Footer type="tree" className="pointer-events-none absolute inset-x-0 bottom-0" />
    </main>
  );
}
