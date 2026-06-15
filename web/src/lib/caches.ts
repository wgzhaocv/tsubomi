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

// 詳細(GET /api/caches/:id):一覧に namespace + キー数(SCAN 概算)を足す。
export type CacheDetail = Cache & {
  namespace: string;
  key_count: number | null;
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

// 詳細(namespace + キー数)。キー数は valkey の SCAN 概算なので毎回取り直す。
export function useCache(id: string) {
  return useQuery({
    queryKey: cacheKeys.detail(id),
    queryFn: async (): Promise<CacheDetail> => {
      const res = await fetch(`/api/caches/${id}`);
      if (!res.ok) return failBody(res);
      return (await res.json()) as CacheDetail;
    },
  });
}

// リネーム(表示名のみ。namespace・接続文字列は不変)。一覧と詳細を無効化。
export function useRenameCache(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (name: string): Promise<Cache> => {
      const res = await fetch(`/api/caches/${id}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name }),
      });
      if (!res.ok) return failBody(res);
      return (await res.json()) as Cache;
    },
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: cacheKeys.all });
      void qc.invalidateQueries({ queryKey: cacheKeys.detail(id) });
    },
  });
}

// 接続文字列(REDIS_URL)を都度取得する(秘密なのでキャッシュしない)。表示要求時のみ。
export function useRevealUrl() {
  return useMutation({
    mutationFn: async (id: string): Promise<string> => {
      const res = await fetch(`/api/caches/${id}/url`);
      if (!res.ok) return failBody(res);
      return ((await res.json()) as { url: string }).url;
    },
  });
}

// rotate:パスワードを差し替え、新しい接続文字列を返す。rotated_at が変わるので無効化。
export function useRotate() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (id: string): Promise<string> => {
      const res = await fetch(`/api/caches/${id}/rotate`, { method: "POST" });
      if (!res.ok) return failBody(res);
      return ((await res.json()) as { url: string }).url;
    },
    onSuccess: (_url, id) => {
      void qc.invalidateQueries({ queryKey: cacheKeys.all });
      void qc.invalidateQueries({ queryKey: cacheKeys.detail(id) });
    },
  });
}
