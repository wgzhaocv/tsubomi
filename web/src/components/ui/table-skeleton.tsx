import { Skeleton } from "@/components/ui/skeleton";

// テーブルの読み込み中プレースホルダ行。各セルに Skeleton バー(pulse)を出して骨架を保ち、
// データ到着で tbody だけ差し替える(spinner→表の差し替えで起きるレイアウト抖動を防ぐ)。
// 表頭とカード枠は呼び出し側が常時描く。
export function TableSkeletonRows({ cols, rows = 5 }: { cols: number; rows?: number }) {
  const rowKeys = Array.from({ length: rows }, (_, i) => `skeleton-row-${i}`);
  const colKeys = Array.from({ length: cols }, (_, i) => `skeleton-col-${i}`);
  return (
    <>
      {rowKeys.map((rk) => (
        <tr key={rk} className="border-b border-[rgba(61,52,40,0.06)] last:border-0">
          {colKeys.map((ck) => (
            <td key={ck} className="px-4 py-3">
              <Skeleton className="h-4 w-full" />
            </td>
          ))}
        </tr>
      ))}
    </>
  );
}
