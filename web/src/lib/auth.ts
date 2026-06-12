// プレースホルダ UI 用の最小限の auth API ヘルパ。本設計は後で行う。

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
