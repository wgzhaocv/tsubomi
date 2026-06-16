// テーブルの読み込み中プレースホルダ行。spinner ではなく各セルに「—」を出して骨架を保ち、
// データ到着でレイアウトが大きく跳ねないようにする(リスト系画面で共通利用)。
// 表頭とカード枠は呼び出し側が常時描き、tbody だけ「これ ↔ 実データ」を差し替える。
export function TableSkeletonRows({ cols, rows = 5 }: { cols: number; rows?: number }) {
  const rowKeys = Array.from({ length: rows }, (_, i) => `skeleton-row-${i}`);
  const colKeys = Array.from({ length: cols }, (_, i) => `skeleton-col-${i}`);
  return (
    <>
      {rowKeys.map((rk) => (
        <tr key={rk} className="border-b border-[rgba(61,52,40,0.06)] last:border-0">
          {colKeys.map((ck) => (
            <td key={ck} className="px-4 py-3 text-sm text-muted-foreground/40">
              —
            </td>
          ))}
        </tr>
      ))}
    </>
  );
}
