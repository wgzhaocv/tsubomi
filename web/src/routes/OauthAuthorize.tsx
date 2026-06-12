import { useEffect, useState } from "react";
import { useSearchParams } from "react-router";

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
    <main className="flex min-h-dvh flex-col items-center justify-center gap-6 bg-background p-8 text-foreground">
      <h1 className="text-2xl font-semibold">tbm CLI 認可</h1>

      {loading && <p className="text-muted-foreground">…</p>}
      {error && <p className="max-w-md text-sm text-red-500">{error}</p>}

      {!loading && !me && (
        <div className="flex flex-col items-center gap-3">
          <p className="text-sm text-muted-foreground">
            先にログインしてから、もう一度 <code>tbm login</code> を実行してください。
          </p>
          <a
            href="/api/auth/google/start"
            className="rounded-md bg-primary px-4 py-2 text-primary-foreground transition hover:opacity-90"
          >
            Sign in with Google
          </a>
        </div>
      )}

      {me && (
        <div className="flex flex-col items-center gap-4">
          <p className="text-sm text-muted-foreground">
            <span className="font-medium text-foreground">{me.email}</span> として
            tbm CLI({params.get("hint") ?? "cli"})にアクセスを許可しますか?
          </p>
          <button
            onClick={() => void approve()}
            className="rounded-md bg-primary px-6 py-2 text-primary-foreground transition hover:opacity-90"
          >
            許可する
          </button>
        </div>
      )}
    </main>
  );
}
