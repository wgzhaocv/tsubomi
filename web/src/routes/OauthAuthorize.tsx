import { useEffect, useState } from "react";
import { useSearchParams } from "react-router";

import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { fetchMe, type Me } from "@/lib/auth";

// PKCE ステップ 2:`tbm login` がこのページを開く。ログイン済みユーザが
// 確認したら、クエリパラメータを /api/oauth/authorize(session 認証)に
// POST し、返ってきた redirect_to へ遷移する。遷移先がコードを表示し、
// ユーザはそれを CLI に貼り戻す。
export default function OauthAuthorize() {
  const [params] = useSearchParams();
  const [me, setMe] = useState<Me | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    fetchMe()
      .then(setMe)
      .catch((e: unknown) => setError(String(e)))
      .finally(() => setLoading(false));
  }, []);

  async function approve() {
    setError(null);
    const body = {
      response_type: params.get("response_type") ?? "",
      client_id: params.get("client_id") ?? "",
      redirect_uri: params.get("redirect_uri") ?? "",
      code_challenge: params.get("code_challenge") ?? "",
      code_challenge_method: params.get("code_challenge_method") ?? "",
      state: params.get("state") ?? "",
      hint: params.get("hint") ?? undefined,
    };
    const res = await fetch("/api/oauth/authorize", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!res.ok) {
      setError(`authorize failed: ${res.status} ${await res.text()}`);
      return;
    }
    const { redirect_to } = (await res.json()) as { redirect_to: string };
    window.location.href = redirect_to;
  }

  return (
    <main className="flex min-h-dvh flex-col items-center justify-center gap-6 p-8 text-foreground">
      <PageMeta title="tbm CLI 認可" />
      <h1 className="text-3xl font-extrabold tracking-tight">tbm CLI 認可</h1>

      <Card className="w-full max-w-md">
        <CardContent className="flex flex-col items-center gap-4">
          {loading && <p className="text-muted-foreground">…</p>}
          {error && (
            <p className="w-full text-center text-sm wrap-break-word text-destructive">{error}</p>
          )}

          {!loading && !me && (
            <>
              <p className="text-center text-sm text-muted-foreground">
                先にログインしてから、もう一度{" "}
                <code className="rounded-md bg-card/80 px-1.5 py-0.5 text-[0.85em] font-bold text-foreground">
                  tbm login
                </code>{" "}
                を実行してください。
              </p>
              <Button asChild type="primary" block>
                <a href="/api/auth/google/start">Google でログイン</a>
              </Button>
            </>
          )}

          {me && (
            <>
              <p className="text-center text-sm text-muted-foreground">
                <span className="font-bold text-foreground">{me.email}</span> として tbm CLI(
                {params.get("hint") ?? "cli"})に アクセスを許可しますか?
              </p>
              <Button
                type="primary"
                block
                onClick={() => {
                  void approve();
                }}
              >
                許可する
              </Button>
            </>
          )}
        </CardContent>
      </Card>
    </main>
  );
}
