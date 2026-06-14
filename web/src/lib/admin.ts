import { useQuery } from "@tanstack/react-query";

import { RESOURCES } from "@/lib/resources";

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
 * ドリフトしないように。activity(kind=null)は除く。 */
export const KIND_LABEL: Record<string, string> = Object.fromEntries(
  RESOURCES.filter((r) => r.kind).map((r) => [r.kind as string, r.label]),
);
