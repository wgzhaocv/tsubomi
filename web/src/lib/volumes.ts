import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

// volume リソースのサーバ状態。databases.ts と同じ作法:生の fetch + それを包む
// TanStack Query フック。一覧・ディレクトリ列挙は Query が単一の真実源。
// ファイル操作(作成/削除/移動/アップロード)後は該当 volume の列挙を無効化する。
//
// 假根内のパスは API では `?path=` クエリで渡す(URLSearchParams が値をエンコード)。
// 画面側(URL)のパスは React Router の splat で持つ(VolumeFileBrowser)。

export type Volume = {
  id: string;
  display_name: string;
  anon_seq: number;
  created_at: string;
};

export type FileEntry = {
  name: string;
  is_dir: boolean;
  size: number;
  modified: string | null;
};

export type ListDir = {
  path: string;
  entries: FileEntry[];
};

export type VolumeUsage = {
  size_bytes: number;
  file_count: number;
  dir_count: number;
  // 走査が時間予算で打ち切られた = 値は下限(UI は「≥」表示)。
  truncated: boolean;
};

export const volumeKeys = {
  all: ["volumes"] as const,
  detail: (id: string) => ["volumes", id] as const,
  files: (id: string, path: string) => ["volumes", id, "files", path] as const,
  usage: (id: string) => ["volumes", id, "usage"] as const,
};

// バイト数を人間可読に(ファイルブラウザ・概要で共用)。
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(1)} ${units[i]}`;
}

// エラー本文(サーバは AppError の日本語メッセージを text で返す)を投げる。
async function failBody(res: Response): Promise<never> {
  const body = await res.text().catch(() => "");
  throw new Error(body || `HTTP ${res.status}`);
}

// `/api/volumes/:id<sub>?path=<encoded>` を組む。値のエンコードは URLSearchParams 任せ。
function filesUrl(id: string, sub: string, path: string): string {
  const qs = new URLSearchParams({ path }).toString();
  return `/api/volumes/${id}${sub}?${qs}`;
}

// ===== リソース CRUD =====

async function fetchVolumes(): Promise<Volume[]> {
  const res = await fetch("/api/volumes");
  if (!res.ok) return failBody(res);
  return (await res.json()) as Volume[];
}

export function useVolumes() {
  return useQuery({ queryKey: volumeKeys.all, queryFn: fetchVolumes });
}

export function useCreateVolume() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (name: string): Promise<Volume> => {
      const res = await fetch("/api/volumes", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name }),
      });
      if (!res.ok) return failBody(res);
      return (await res.json()) as Volume;
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: volumeKeys.all }),
  });
}

export function useDeleteVolume() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (id: string): Promise<void> => {
      const res = await fetch(`/api/volumes/${id}`, { method: "DELETE" });
      if (!res.ok) return failBody(res);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: volumeKeys.all }),
  });
}

// リネーム(表示名のみ。host_path・ファイルは不変)。一覧の名前が変わるので一覧を無効化。
export function useRenameVolume(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (name: string): Promise<Volume> => {
      const res = await fetch(`/api/volumes/${id}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name }),
      });
      if (!res.ok) return failBody(res);
      return (await res.json()) as Volume;
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: volumeKeys.all }),
  });
}

// ===== ファイル操作(全て safe_path を通る)=====

// ディレクトリ列挙。path は假根からの相対(空 = ルート)。
export function useListDir(id: string, path: string) {
  return useQuery({
    queryKey: volumeKeys.files(id, path),
    queryFn: async (): Promise<ListDir> => {
      const res = await fetch(filesUrl(id, "/files", path));
      if (!res.ok) return failBody(res);
      return (await res.json()) as ListDir;
    },
  });
}

// 使用量(概要ページ用)。キーは detail(id) の prefix 配下なので、ファイル操作の
// 無効化(detail(id))で自動的に取り直される。
export function useVolumeUsage(id: string) {
  return useQuery({
    queryKey: volumeKeys.usage(id),
    queryFn: async (): Promise<VolumeUsage> => {
      const res = await fetch(`/api/volumes/${id}/usage`);
      if (!res.ok) return failBody(res);
      return (await res.json()) as VolumeUsage;
    },
    // 60 秒は再計算しない(画面を行き来しても走盘を繰り返さない)。ファイル操作で
    // detail(id) が無効化されればその時に取り直す(staleTime より invalidate が優先)。
    staleTime: 60_000,
  });
}

// アップロード(生バイトを PUT)。fetch では上传進度が取れないので XHR を使う。
// File を直接 send するのでブラウザがディスクからストリームし、JS メモリに全部は
// 載らない(大きいファイルでも安全)。onProgress(0–100)で進捗を返す。
export function useUpload(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      path,
      file,
      onProgress,
    }: {
      path: string;
      file: File;
      onProgress?: (pct: number) => void;
    }): Promise<void> =>
      new Promise<void>((resolve, reject) => {
        const xhr = new XMLHttpRequest();
        xhr.open("PUT", filesUrl(id, "/files", path));
        xhr.upload.onprogress = (e) => {
          if (e.lengthComputable) onProgress?.(Math.round((e.loaded / e.total) * 100));
        };
        xhr.onload = () => {
          if (xhr.status >= 200 && xhr.status < 300) {
            onProgress?.(100);
            resolve();
          } else {
            // サーバの 4xx 本文(日本語メッセージ)をそのままエラーに。
            reject(new Error(xhr.responseText || `HTTP ${xhr.status}`));
          }
        };
        xhr.onerror = () => reject(new Error("ネットワークエラー(アップロード)"));
        xhr.send(file);
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: volumeKeys.detail(id) }),
  });
}

export function useMkdir(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (path: string): Promise<void> => {
      const res = await fetch(filesUrl(id, "/dirs", path), { method: "POST" });
      if (!res.ok) return failBody(res);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: volumeKeys.detail(id) }),
  });
}

export function useDeleteEntry(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (path: string): Promise<void> => {
      const res = await fetch(filesUrl(id, "/files", path), { method: "DELETE" });
      if (!res.ok) return failBody(res);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: volumeKeys.detail(id) }),
  });
}

export function useMove(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async ({ from, to }: { from: string; to: string }): Promise<void> => {
      const res = await fetch(`/api/volumes/${id}/move`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ from, to }),
      });
      if (!res.ok) return failBody(res);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: volumeKeys.detail(id) }),
  });
}

// ダウンロード:ブラウザネイティブの保存に任せる(フックではない)。download
// エンドポイントへ直接 <a download> で誘導するので、ブラウザがディスクへ逐次書き
// (大きいファイルでも JS メモリに blob を載せない)。同一オリジン GET なので
// session cookie も自動で付く。失敗時(4xx)はブラウザ任せ(エラー本文が落ちる)。
export function downloadFile(id: string, path: string): void {
  const a = document.createElement("a");
  a.href = filesUrl(id, "/files/download", path);
  a.download = path.split("/").pop() || "download";
  document.body.appendChild(a);
  a.click();
  a.remove();
}

// プレビュー用 URL。download エンドポイントに inline=true を付ける。サーバは inline 時
// 推測 MIME + Content-Disposition: inline + CSP sandbox で返す(新しいタブで開く想定)。
export function previewUrl(id: string, path: string): string {
  const qs = new URLSearchParams({ path, inline: "true" }).toString();
  return `/api/volumes/${id}/files/download?${qs}`;
}
