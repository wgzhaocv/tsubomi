import { useEffect, useState } from "react";
import { Link } from "react-router";

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
    <main className="flex min-h-dvh flex-col items-center justify-center gap-6 bg-background p-8 text-foreground">
      <h1 className="text-4xl font-semibold tracking-tight">🌷 つぼみ</h1>
      <p className="text-sm text-muted-foreground">internal PaaS — placeholder UI</p>

      {loading && <p className="text-muted-foreground">…</p>}
      {error && <p className="text-sm text-red-500">{error}</p>}

      {!loading && !me && (
        <a
          href="/api/auth/google/start"
          className="rounded-md bg-primary px-4 py-2 text-primary-foreground transition hover:opacity-90"
        >
          Sign in with Google
        </a>
      )}

      {me && (
        <div className="flex flex-col items-center gap-3">
          <div className="flex items-center gap-3">
            {me.avatar_url && (
              <img src={me.avatar_url} alt="" className="h-8 w-8 rounded-full" />
            )}
            <span>
              {me.name ?? me.email}
              <span className="ml-2 text-xs text-muted-foreground">({me.role})</span>
            </span>
          </div>
          <Link
            to="/cli"
            className="text-sm text-muted-foreground underline-offset-4 hover:underline"
          >
            tbm CLI をインストール →
          </Link>
          <button
            onClick={() => {
              void logout().then(() => setMe(null));
            }}
            className="text-sm text-muted-foreground underline-offset-4 hover:underline"
          >
            Logout
          </button>
        </div>
      )}
    </main>
  );
}
