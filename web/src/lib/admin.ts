import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { authKeys } from "@/lib/auth";
import { RESOURCES } from "@/lib/resources";
import { formatBytes } from "@/lib/volumes";

// 使用量の表示(種別で単位が違う):cache は key 数(個)、それ以外は bytes 整形(§4.2)。
// overview(総覧)と ranking で共有する。null は「取得不能」。
export function formatUsageByKind(kind: string, value: number | null): string {
  if (value == null) return "—";
  return kind === "cache" ? `${value} 個` : formatBytes(value);
}

// owner ガバナンスの管制面(M4 S1)。ipblock.ts と同じ作法:生 fetch + TanStack Query。
// 匿名化済み(設計 v2 §7):真名は出すが資源は匿名番号、内容は出さない。読み取り専用。
// 後端が owner + session を毎回検証するので、ここは UX(画面でも弾くが本丸は後端)。

// 指標採集はやや重い(service stats は 1 件 ~1 秒)ので少し長めにキャッシュし、
// 総覧↔ランキングの行き来や種別タブ切替で毎回再取得しないようにする。
const STALE_MS = 30_000;

export type AdminResourceRow = {
  resource_id: string;
  owner_name: string;
  kind: string;
  anon_label: string;
  /** 使用量(bytes)。database=存储 / volume=占用 / service=稼働中内存。取得不能は null。 */
  usage_bytes: number | null;
  /** service のみ:CPU 使用率(%)。 */
  cpu_pct: number | null;
  /** service のみ:稼働中か。 */
  running: boolean | null;
};

export type AdminOverviewKind = {
  kind: string;
  count: number;
  total_usage_bytes: number;
};

export type AdminOverview = {
  user_count: number;
  kinds: AdminOverviewKind[];
};

export const adminKeys = {
  overview: ["admin", "overview"] as const,
  ranking: ["admin", "ranking"] as const,
};

// サーバは AppError の日本語メッセージを text で返す。そのまま throw する。
async function failBody(res: Response): Promise<never> {
  const body = await res.text().catch(() => "");
  throw new Error(body || `HTTP ${res.status}`);
}

async function fetchOverview(): Promise<AdminOverview> {
  const res = await fetch("/api/admin/overview");
  if (!res.ok) return failBody(res);
  return (await res.json()) as AdminOverview;
}

// 全件を 1 回取り、種別フィルタは画面側で行う(タブ切替ごとの再取得 = 重い再採集を避ける)。
async function fetchRanking(): Promise<AdminResourceRow[]> {
  const res = await fetch("/api/admin/ranking");
  if (!res.ok) return failBody(res);
  return (await res.json()) as AdminResourceRow[];
}

export function useAdminOverview() {
  return useQuery({ queryKey: adminKeys.overview, queryFn: fetchOverview, staleTime: STALE_MS });
}

export function useAdminRanking() {
  return useQuery({ queryKey: adminKeys.ranking, queryFn: fetchRanking, staleTime: STALE_MS });
}

/** kind → 日本語ラベル(画面表示用)。RESOURCES(単一の真実源)から導出 — ラベルが
 * ドリフトしないように。kind=null の項目(ゴミ箱など)は除く。 */
export const KIND_LABEL: Record<string, string> = Object.fromEntries(
  RESOURCES.filter((r) => r.kind).map((r) => [r.kind as string, r.label]),
);

// 最後の砦(S3):owner が他人の資源を停止 / 削除。二段確認 —
//   1 回目(code なし)→ サーバが owner にメールでコードを送り { code_required: true }。
//   2 回目(code あり)→ 検証して実行し { code_required: false }(実行済みなので一覧を無効化)。
export type AdminAction = "stop" | "delete";
export type AdminActionInput = { id: string; action: AdminAction; code?: string };

export function useAdminAction() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async ({
      id,
      action,
      code,
    }: AdminActionInput): Promise<{ code_required: boolean }> => {
      const res = await fetch(`/api/admin/resources/${id}/${action}`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ code: code ?? null }),
      });
      if (!res.ok) return failBody(res);
      return (await res.json()) as { code_required: boolean };
    },
    onSuccess: (data) => {
      // 実行されたのは 2 段目(code_required=false)だけ。1 段目はコード送信のみ = 状態不変。
      if (!data.code_required) {
        qc.invalidateQueries({ queryKey: adminKeys.ranking });
        qc.invalidateQueries({ queryKey: adminKeys.overview });
      }
    },
  });
}

// 監査ログ閲覧(S4)。キーセット分頁(id DESC)を useInfiniteQuery で「もっと読む」。
// action は前方一致フィルタ(例 "owner." で代理操作だけ)。
export type AuditEntry = {
  id: number;
  created_at: string;
  action: string;
  actor_name: string | null;
  target_user_name: string | null;
  target_resource: string | null;
  detail: unknown;
};

const AUDIT_PAGE = 50;

async function fetchAudit(cursor: number | null, action: string): Promise<AuditEntry[]> {
  const p = new URLSearchParams({ limit: String(AUDIT_PAGE) });
  if (cursor != null) p.set("cursor", String(cursor));
  if (action) p.set("action", action);
  const res = await fetch(`/api/admin/audit?${p}`);
  if (!res.ok) return failBody(res);
  return (await res.json()) as AuditEntry[];
}

export function useAuditLog(action: string) {
  return useInfiniteQuery({
    queryKey: ["admin", "audit", action],
    queryFn: ({ pageParam }) => fetchAudit(pageParam, action),
    initialPageParam: null as number | null,
    // 1 頁が満杯なら続きがある可能性 → 最後の id を次カーソルに。満杯未満 = 終端。
    getNextPageParam: (last) => (last.length === AUDIT_PAGE ? last[last.length - 1].id : undefined),
    staleTime: STALE_MS,
  });
}

// 共有パスワード viewer(S5、web 専用)。設計 v2 §7「見るは共有密码」。
// login = 任意のログインユーザが共有パスワードを入れて只读 grant(8h)を得る。
// status / set = owner のみ(後端が require_owner_web)。
export type ViewerStatus = {
  set: boolean;
  updated_at: string | null;
  updated_by_name: string | null;
};

export const viewerKeys = {
  status: ["admin", "viewer", "status"] as const,
};

// 共有パスワードを入れて閲覧 grant を得る。成功したら me を無効化(is_viewer が翻る)。
export function useViewerLogin() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (password: string): Promise<void> => {
      const res = await fetch("/api/admin/viewer/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ password }),
      });
      if (!res.ok) return failBody(res);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: authKeys.me }),
  });
}

// 共有パスワードの設定状態(owner の設定ページ表示用)。
async function fetchViewerStatus(): Promise<ViewerStatus> {
  const res = await fetch("/api/admin/viewer/password");
  if (!res.ok) return failBody(res);
  return (await res.json()) as ViewerStatus;
}

export function useViewerStatus() {
  return useQuery({ queryKey: viewerKeys.status, queryFn: fetchViewerStatus, staleTime: STALE_MS });
}

// 共有パスワードを設定 / リセット(owner)。旧 grant は後端で全失効する。
export function useSetViewerPassword() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (password: string): Promise<ViewerStatus> => {
      const res = await fetch("/api/admin/viewer/password", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ password }),
      });
      if (!res.ok) return failBody(res);
      return (await res.json()) as ViewerStatus;
    },
    // サーバが最新状態を返すので、再 GET せずキャッシュに直接書く(LEFT JOIN の一往復を節約)。
    onSuccess: (data) => qc.setQueryData(viewerKeys.status, data),
  });
}
