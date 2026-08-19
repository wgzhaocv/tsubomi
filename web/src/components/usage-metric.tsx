import { Skeleton } from "@/components/ui/skeleton";

// 使用量の見せ方(バー + 「使用中 / 上限」の数値)の単一真源。管理概要のホスト指標と
// service 詳細の資源使用量が同じ意匠で並ぶように、ここから両方が引く。
// (意匠の出どころは VolumeFileBrowser のアップロード進捗バー。)

// 用量バー。pct が null(取得不能)なら 0 幅で描く。
export function UsageBar({ pct }: { pct: number | null }) {
  return (
    <div className="h-2 w-full overflow-hidden rounded-full bg-[rgba(196,184,158,0.3)]">
      <div
        className="h-full rounded-full bg-[#0CC0B5] transition-[width] duration-150 ease-out"
        style={{ width: `${Math.min(100, Math.max(0, pct ?? 0))}%` }}
      />
    </div>
  );
}

/**
 * ラベル + 右寄せの数値 + バーの 1 行。loading 中は Skeleton に差し替える。
 * `pct` は null = 取得不能(0 幅バー)、**undefined = 分母が無いのでバー自体を出さない**
 * (0 幅バーだと「使っていない」に見えるため)。service の CPU は個別上限が無くても
 * ホスト全体を天井にするので分母は基本ある — undefined になるのはコア数が取れない時だけ。
 */
export function MetricRow({
  label,
  pct,
  detail,
  loading,
  mono = false,
}: {
  label: string;
  pct?: number | null;
  detail: string;
  loading: boolean;
  /** ラベルを等幅・切り詰め表示(パス / URL などの生値向け。統計の内訳が使う)。 */
  mono?: boolean;
}) {
  // 分母が無い行(上限なしの CPU など)は最初からバーを出さない。loading 中だけ
  // Skeleton のバーを見せて後から消すと行の高さが跳ねる。
  const hasBar = pct !== undefined;
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-baseline justify-between gap-3">
        <span
          className={
            mono
              ? "min-w-0 truncate font-mono text-[13px] font-semibold text-foreground"
              : "min-w-0 truncate text-sm font-bold text-foreground"
          }
          title={label}
        >
          {label}
        </span>
        {loading ? (
          <Skeleton className="h-4 w-20" />
        ) : (
          <span className="font-mono text-sm font-bold text-[#0b9c93]">{detail}</span>
        )}
      </div>
      {hasBar &&
        (loading ? <Skeleton className="h-2 w-full rounded-full" /> : <UsageBar pct={pct} />)}
    </div>
  );
}
