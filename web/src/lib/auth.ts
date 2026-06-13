import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

// 認証まわりのサーバ状態。生の fetch(fetchMe / logout)と、それを包む
// TanStack Query フック(useMeQuery / useLogout)をここに集約する。
// 「今ログインしているのは誰か」はサーバ状態なので Query が単一の真実源:
// 必要なコンポーネントは useMeQuery() を直接呼ぶ(props で配らない)。

export type Me = {
  user_id: string;
  email: string;
  name: string | null;
  avatar_url: string | null;
  role: "user" | "owner";
};

export const authKeys = {
  me: ["auth", "me"] as const,
};

export async function fetchMe(): Promise<Me | null> {
  const res = await fetch("/api/auth/me");
  if (res.status === 401) return null; // 未ログインはエラーではなく null
  if (!res.ok) throw new Error(`/api/auth/me failed: ${res.status}`);
  return (await res.json()) as Me;
}

async function postLogout(): Promise<void> {
  await fetch("/api/auth/logout", { method: "POST" });
}

// ログインユーザ。未ログインは data=null。401 は正常系なので retry しない。
export function useMeQuery() {
  return useQuery({
    queryKey: authKeys.me,
    queryFn: fetchMe,
    staleTime: 5 * 60_000,
    retry: false,
  });
}

// ログアウト。成功したら me キャッシュを null に倒す(守衛が /login へ送る)。
export function useLogout() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: postLogout,
    onSuccess: () => {
      qc.setQueryData(authKeys.me, null);
    },
  });
}
