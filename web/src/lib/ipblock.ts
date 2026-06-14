import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

// 会社 IP 許可リストのサーバ状態(owner 専用)。volumes.ts / auth.ts と同じ作法:
// 生の fetch + それを包む TanStack Query フック。一覧が単一の真実源で、追加 / 削除の
// 成功後に無効化する。
//
// 意味は「許可リスト」:
//   * 空        = 制限なし(全 IP 許可)。設定するまで誰でも service に繋がる。
//   * 1 件以上  = 列挙した CIDR だけが service に到達でき、他は traefik で遮断。
// 反映はライブ(平台が traefik の動的設定を書き直し、middleware がホットリロード)。

export type IpAllowEntry = {
  id: string;
  cidr: string;
  note: string;
  created_at: string;
};

export const ipAllowKeys = {
  all: ["ip-allowlist"] as const,
};

// サーバは AppError の日本語メッセージを text で返す。それをそのまま throw する。
async function failBody(res: Response): Promise<never> {
  const body = await res.text().catch(() => "");
  throw new Error(body || `HTTP ${res.status}`);
}

async function fetchEntries(): Promise<IpAllowEntry[]> {
  const res = await fetch("/api/ip-allowlist");
  if (!res.ok) return failBody(res);
  return (await res.json()) as IpAllowEntry[];
}

export function useIpAllowlist() {
  return useQuery({ queryKey: ipAllowKeys.all, queryFn: fetchEntries });
}

export function useAddIpAllow() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (input: { cidr: string; note: string }): Promise<IpAllowEntry> => {
      const res = await fetch("/api/ip-allowlist", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(input),
      });
      if (!res.ok) return failBody(res);
      return (await res.json()) as IpAllowEntry;
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ipAllowKeys.all }),
  });
}

export function useRemoveIpAllow() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (id: string): Promise<void> => {
      const res = await fetch(`/api/ip-allowlist/${id}`, { method: "DELETE" });
      if (!res.ok) return failBody(res);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ipAllowKeys.all }),
  });
}
