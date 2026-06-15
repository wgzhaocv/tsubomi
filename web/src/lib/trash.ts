import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { cacheKeys } from "@/lib/caches";
import { dbKeys } from "@/lib/databases";
import { volumeKeys } from "@/lib/volumes";

// ゴミ箱(4 種リソース共通)。databases.ts / volumes.ts と同じ作法。
// 復元 / 完全削除のあとは、ゴミ箱一覧と各リソース一覧を無効化する
// (復元するとリソースが一覧へ戻るため)。

export type TrashItem = {
  id: string;
  kind: string; // service | database | cache | volume
  display_name: string;
  deleted_at: string;
  purge_after: string | null;
};

export const trashKeys = { all: ["trash"] as const };

async function failBody(res: Response): Promise<never> {
  const body = await res.text().catch(() => "");
  throw new Error(body || `HTTP ${res.status}`);
}

async function fetchTrash(): Promise<TrashItem[]> {
  const res = await fetch("/api/trash");
  if (!res.ok) return failBody(res);
  return (await res.json()) as TrashItem[];
}

export function useTrash() {
  return useQuery({ queryKey: trashKeys.all, queryFn: fetchTrash });
}

// 復元・完全削除で共通の無効化(ゴミ箱 + 各リソース一覧 + dashboard の resources)。
function useTrashMutation(run: (id: string) => Promise<void>) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: run,
    // 1 つの Promise.all を返す(mutation が await する → floating にならない)。
    onSuccess: () =>
      Promise.all([
        qc.invalidateQueries({ queryKey: trashKeys.all }),
        qc.invalidateQueries({ queryKey: dbKeys.all }),
        qc.invalidateQueries({ queryKey: volumeKeys.all }),
        qc.invalidateQueries({ queryKey: cacheKeys.all }),
        qc.invalidateQueries({ queryKey: ["resources"] }),
      ]),
  });
}

export function useRestore() {
  return useTrashMutation(async (id) => {
    const res = await fetch(`/api/trash/${id}/restore`, { method: "POST" });
    if (!res.ok) return failBody(res);
  });
}

export function usePurge() {
  return useTrashMutation(async (id) => {
    const res = await fetch(`/api/trash/${id}`, { method: "DELETE" });
    if (!res.ok) return failBody(res);
  });
}
