import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

// cache(valkey)リソースのサーバ状態。lib/databases.ts と同型:生の fetch + それを包む
// TanStack Query フック。一覧は Query が単一の真実源(props で配らない)。
// rotate / url(秘密)/ rename は S3 で足す。

export type Cache = {
  id: string;
  display_name: string;
  anon_seq: number;
  created_at: string;
  rotated_at: string | null;
};

export const cacheKeys = {
  all: ["caches"] as const,
  detail: (id: string) => ["caches", id] as const,
};

// エラー本文(サーバは AppError の日本語メッセージを text で返す)を投げる。
async function failBody(res: Response): Promise<never> {
  const body = await res.text().catch(() => "");
  throw new Error(body || `HTTP ${res.status}`);
}

async function fetchCaches(): Promise<Cache[]> {
  const res = await fetch("/api/caches");
  if (!res.ok) return failBody(res);
  return (await res.json()) as Cache[];
}

export function useCaches() {
  return useQuery({ queryKey: cacheKeys.all, queryFn: fetchCaches });
}

export function useCreateCache() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (name: string): Promise<Cache> => {
      const res = await fetch("/api/caches", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name }),
      });
      if (!res.ok) return failBody(res);
      return (await res.json()) as Cache;
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: cacheKeys.all }),
  });
}

export function useDeleteCache() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (id: string): Promise<void> => {
      const res = await fetch(`/api/caches/${id}`, { method: "DELETE" });
      if (!res.ok) return failBody(res);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: cacheKeys.all }),
  });
}
