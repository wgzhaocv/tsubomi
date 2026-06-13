import { useEffect, useState } from "react";
import { Link } from "react-router";

import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { fetchMe, logout, type Me } from "@/lib/auth";

// プレースホルダのダッシュボード:ログイン状態のみ。本設計は後で行う。
export default function Home() {
  const [me, setMe] = useState<Me | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    fetchMe()
      .then(setMe)
      .catch((e: unknown) => setError(String(e)))
      .finally(() => setLoading(false));
  }, []);

  return (
    <main className="flex min-h-dvh flex-col items-center justify-center gap-7 p-8 text-foreground">
      <PageMeta />
      <div className="flex flex-col items-center gap-1">
        <h1 className="flex items-center gap-3 text-5xl font-extrabold tracking-tight">
          <img src="/logo.png" alt="" className="h-12 w-auto" />
          つぼみ
        </h1>
        <p className="text-sm font-bold text-foreground/70">社内 PaaS プラットフォーム</p>
      </div>

      <Card className="w-full max-w-sm">
        <CardContent className="flex flex-col items-center gap-4">
          {loading && <p className="text-muted-foreground">…</p>}
          {error && <p className="text-sm text-destructive">{error}</p>}

          {!loading && !me && (
            <>
              <p className="text-center text-sm text-muted-foreground">
                Google アカウントでログインしてください。
              </p>
              <Button asChild type="primary" block>
                <a href="/api/auth/google/start">Google でログイン</a>
              </Button>
            </>
          )}

          {me && (
            <>
              <div className="flex w-full items-center gap-3">
                {me.avatar_url && (
                  <img
                    src={me.avatar_url}
                    alt=""
                    className="size-10 shrink-0 rounded-full border-2 border-border"
                  />
                )}
                <div className="flex min-w-0 flex-col">
                  <span className="truncate font-bold">{me.name ?? me.email}</span>
                  <span className="truncate text-xs text-muted-foreground">
                    {me.email} · {me.role}
                  </span>
                </div>
              </div>
              <Button asChild type="default" block>
                <Link to="/cli">tbm CLI をインストール →</Link>
              </Button>
              <Button
                type="text"
                size="small"
                onClick={() => {
                  void logout().then(() => setMe(null));
                }}
              >
                ログアウト
              </Button>
            </>
          )}
        </CardContent>
      </Card>
    </main>
  );
}
