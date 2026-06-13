// プレースホルダ UI 用の最小限の auth API ヘルパ。本設計は後で行う。

import { useEffect, useState } from "react";

export type Me = {
  user_id: string;
  email: string;
  name: string | null;
  avatar_url: string | null;
  role: "user" | "owner";
};

export async function fetchMe(): Promise<Me | null> {
  const res = await fetch("/api/auth/me");
  if (res.status === 401) return null;
  if (!res.ok) throw new Error(`/api/auth/me failed: ${res.status}`);
  return (await res.json()) as Me;
}

export async function logout(): Promise<void> {
  await fetch("/api/auth/logout", { method: "POST" });
}

// 共有フック:`/api/auth/me` を 1 回だけ取得し、ログイン状態を返す。
// ログイン守衛(DashboardLayout)・ログイン画面(Login)で共用する。
// setMe を返すので、ログアウト後に呼び出し側で即座に null へ反映できる。
export function useMe() {
  const [me, setMe] = useState<Me | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    // アンマウント後の setState を避けるためのガード(StrictMode の二重実行対策)。
    let alive = true;
    fetchMe()
      .then((m) => {
        if (alive) setMe(m);
      })
      .catch((e: unknown) => {
        if (alive) setError(String(e));
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, []);

  return { me, loading, error, setMe };
}
