import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

// owner 管理のサーバ状態(owner 専用)。ipblock.ts と同じ作法:生 fetch + TanStack Query。
// 一覧が単一の真実源。add / remove はサーバが最新一覧を返すので、再 GET せずキャッシュへ直書き。
// 真相は users.role(後端)。ここは UX — 後端が require_owner_web で守る。

export type AdminOwner = {
  email: string;
  name: string | null;
  is_current: boolean;
  // 既にログイン済み(有効)= true。false = roster にいるが未ログイン、次回ログインで昇格。
  registered: boolean;
};

export const ownerKeys = {
  all: ["admin", "owners"] as const,
};

async function failBody(res: Response): Promise<never> {
  const body = await res.text().catch(() => "");
  throw new Error(body || `HTTP ${res.status}`);
}

async function fetchOwners(): Promise<AdminOwner[]> {
  const res = await fetch("/api/admin/owners");
  if (!res.ok) return failBody(res);
  return (await res.json()) as AdminOwner[];
}

export function useOwners() {
  return useQuery({ queryKey: ownerKeys.all, queryFn: fetchOwners });
}

// add / remove は URL 以外まったく同じ(email を POST → 最新一覧を受けてキャッシュへ直書き)。
// 1 つの hook ファクトリに畳む(rules-of-hooks のため `use` 接頭辞)。
function useOwnerMutation(url: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (email: string): Promise<AdminOwner[]> => {
      const res = await fetch(url, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email }),
      });
      if (!res.ok) return failBody(res);
      return (await res.json()) as AdminOwner[];
    },
    onSuccess: (data) => qc.setQueryData(ownerKeys.all, data),
    // 失敗時は別クライアントが並行変更した可能性があるので一覧を取り直す(canAdd も新鮮に)。
    onError: () => qc.invalidateQueries({ queryKey: ownerKeys.all }),
  });
}

export function useAddOwner() {
  return useOwnerMutation("/api/admin/owners");
}

export function useRemoveOwner() {
  return useOwnerMutation("/api/admin/owners/remove");
}
