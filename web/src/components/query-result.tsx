import type { ReactNode } from "react";

import type { QueryResult } from "@/lib/databases";

// クエリ結果 / テーブルデータ共通の表(カラム + 行)。値は text(NULL は薄字)。
// SQL コンソール(DatabaseEditor)とテーブル閲覧(DatabaseTables の DATA)で共用。
export function ResultTable({
  result,
  empty,
}: {
  result: QueryResult;
  // 列はあるが行が 0 のときの差し込み(既定は「行がありません」)。
  empty?: ReactNode;
}) {
  // SELECT 以外(INSERT/CREATE 等)は列が無い ⇒ 成功メッセージだけ。
  if (result.columns.length === 0) {
    return <p className="text-sm font-semibold text-[#11a89b]">OK(返す行はありません)。</p>;
  }

  return (
    <div className="flex flex-col gap-1.5">
      <div className="overflow-auto rounded-2xl border-2 border-[#c4b89e]">
        <table className="w-full border-collapse text-sm">
          <thead>
            <tr className="bg-accent">
              {result.columns.map((c, ci) => (
                // 列名は重複しうる(JOIN / 別名) ⇒ key には index を混ぜる。
                <th
                  key={`${c}-${ci}`}
                  className="border-b-2 border-[#c4b89e] px-3 py-2 text-left font-bold whitespace-nowrap text-accent-foreground"
                >
                  {c}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {result.rows.length === 0 ? (
              <tr>
                <td
                  colSpan={result.columns.length}
                  className="px-3 py-6 text-center text-sm font-medium text-muted-foreground"
                >
                  {empty ?? "行がありません。"}
                </td>
              </tr>
            ) : (
              result.rows.map((row, ri) => (
                <tr key={ri} className="even:bg-[rgba(196,184,158,0.12)]">
                  {row.map((cell, ci) => (
                    <td
                      key={ci}
                      className="border-b border-[#e8e2d6] px-3 py-1.5 align-top font-medium text-[#725d42]"
                      title={cell ?? undefined}
                    >
                      {/* 表セルの max-width は auto レイアウトでは効かない ⇒ ブロックの
                          内側 div で幅を縛り、長い値を省略表示する(全文は title に出る)。 */}
                      <div className="max-w-100 truncate">
                        {cell === null ? <span className="text-[#c4b89e] italic">NULL</span> : cell}
                      </div>
                    </td>
                  ))}
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>
      <p className="text-xs font-medium text-muted-foreground">
        {result.row_count} 行{result.truncated ? "(上限 1000 行で切り詰め)" : ""}
      </p>
    </div>
  );
}
