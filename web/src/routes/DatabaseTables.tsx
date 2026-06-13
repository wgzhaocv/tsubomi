import { useState } from "react";
import { Columns3, Database, RefreshCw, Search, Table2 } from "lucide-react";
import { Link, NavLink, useParams } from "react-router";

import { ResultTable } from "@/components/query-result";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useTableColumns, useTableRows, useTables } from "@/lib/databases";
import { cn } from "@/lib/utils";

// テーブル閲覧:左にテーブル一覧、右に選択中テーブルの DATA / STRUCTURE。
// 選択中テーブルは URL(/databases/:id/tables/:table)で持ち、深リンク可能。
// データは専用 API ではなく /query 経由(lib/databases の useTables* 参照)。

export default function DatabaseTables() {
  const { id = "", table } = useParams();
  const tables = useTables(id);

  return (
    <div className="grid gap-5 md:grid-cols-[16rem_minmax(0,1fr)]">
      <TableList id={id} selected={table} query={tables} />
      <div className="min-w-0">
        {table ? (
          <TableViewer id={id} table={table} />
        ) : (
          <EmptyRight hasTables={!!tables.data && tables.data.length > 0} />
        )}
      </div>
    </div>
  );
}

// ===== 左:テーブル一覧(検索 + リスト)=====
function TableList({
  id,
  selected,
  query,
}: {
  id: string;
  selected?: string;
  query: ReturnType<typeof useTables>;
}) {
  const [filter, setFilter] = useState("");
  const all = query.data ?? [];
  const shown = filter ? all.filter((t) => t.toLowerCase().includes(filter.toLowerCase())) : all;

  return (
    <aside className="flex max-h-[70vh] flex-col gap-2 rounded-2xl border-2 border-[#e8e2d6] bg-card p-3 md:sticky md:top-6">
      {/* 検索 + 件数 */}
      <div className="flex items-center gap-2 rounded-xl border-2 border-[#c4b89e] bg-[rgb(247,243,223)] px-2.5 py-1.5 focus-within:[outline:2px_solid_#19c8b9] focus-within:outline-offset-1">
        <Search className="size-4 shrink-0 text-[#c4b89e]" />
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="テーブルを検索…"
          spellCheck={false}
          className="min-w-0 flex-1 bg-transparent text-sm font-medium text-[#725d42] outline-none placeholder:text-[#c4b89e]"
        />
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
        {query.isPending && <p className="px-2 py-3 text-sm text-muted-foreground">読み込み中…</p>}
        {query.error && (
          <p className="px-2 py-3 text-sm font-semibold text-[#e05a5a]">{query.error.message}</p>
        )}
        {query.data && all.length === 0 && (
          <p className="px-2 py-3 text-sm font-medium text-muted-foreground">
            テーブルがありません。
          </p>
        )}
        {query.data && all.length > 0 && shown.length === 0 && (
          <p className="px-2 py-3 text-sm font-medium text-muted-foreground">一致なし。</p>
        )}
        <ul className="flex flex-col gap-0.5">
          {shown.map((t) => (
            <li key={t}>
              <NavLink
                to={`/databases/${id}/tables/${encodeURIComponent(t)}`}
                className={cn(
                  "flex min-h-9 items-center gap-2 rounded-xl px-2.5 py-1.5 text-sm font-semibold outline-none transition-colors duration-150 focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-1",
                  t === selected
                    ? "bg-[#0CC0B5] text-[#FFF9E3]"
                    : "text-foreground hover:bg-[rgba(25,200,185,0.1)] hover:text-[#11a89b]",
                )}
              >
                <Table2 className="size-4 shrink-0 opacity-80" />
                <span className="min-w-0 truncate">{t}</span>
              </NavLink>
            </li>
          ))}
        </ul>
      </div>
    </aside>
  );
}

// ===== 右:選択中テーブルの DATA / STRUCTURE =====
function TableViewer({ id, table }: { id: string; table: string }) {
  const [tab, setTab] = useState<"data" | "structure">("data");
  // 表示中の tab だけ取りに行く(隠れた tab の SQL は投げない)。切替時は
  // staleTime 内ならキャッシュ命中で即表示なので、行き来は体感即座のまま。
  const rows = useTableRows(id, tab === "data" ? table : undefined);
  const columns = useTableColumns(id, tab === "structure" ? table : undefined);

  const active = tab === "data" ? rows : columns;
  const refresh = () => void active.refetch();

  return (
    <div className="flex flex-col gap-3">
      {/* ヘッダ:テーブル名 + DATA/STRUCTURE 切替 + 再読込 */}
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-2 text-base font-bold text-foreground">
          <Table2 className="size-5 shrink-0 text-[#11a89b]" />
          <span className="min-w-0 truncate">{table}</span>
        </div>
        <div className="flex items-center gap-2">
          <div className="flex gap-1 rounded-2xl bg-[rgba(196,184,158,0.18)] p-1">
            <Seg
              active={tab === "data"}
              onClick={() => setTab("data")}
              icon={<Table2 className="size-4" />}
            >
              DATA
            </Seg>
            <Seg
              active={tab === "structure"}
              onClick={() => setTab("structure")}
              icon={<Columns3 className="size-4" />}
            >
              STRUCTURE
            </Seg>
          </div>
          <Button
            type="text"
            size="small"
            aria-label="再読込"
            icon={<RefreshCw className={cn("size-4", active.isFetching && "animate-spin")} />}
            onClick={refresh}
          />
        </div>
      </div>

      {/* 本文 */}
      {tab === "data" ? <DataPane query={rows} /> : <StructurePane query={columns} />}
    </div>
  );
}

// DATA タブ:先頭 100 行。
function DataPane({ query }: { query: ReturnType<typeof useTableRows> }) {
  if (query.isPending) return <Loading />;
  if (query.error)
    return <p className="text-sm font-semibold text-[#e05a5a]">{query.error.message}</p>;
  if (!query.data) return null;
  return <ResultTable result={query.data} empty="このテーブルは空です。" />;
}

// STRUCTURE タブ:列定義(名前 / 型 / NULL 可否 / 既定値)。
function StructurePane({ query }: { query: ReturnType<typeof useTableColumns> }) {
  if (query.isPending) return <Loading />;
  if (query.error)
    return <p className="text-sm font-semibold text-[#e05a5a]">{query.error.message}</p>;
  const cols = query.data ?? [];
  if (cols.length === 0)
    return <p className="text-sm font-medium text-muted-foreground">列がありません。</p>;

  return (
    <div className="overflow-auto rounded-2xl border-2 border-[#c4b89e]">
      <table className="w-full border-collapse text-sm">
        <thead>
          <tr className="bg-accent">
            {["列名", "型", "NULL 可", "既定値"].map((h) => (
              <th
                key={h}
                className="border-b-2 border-[#c4b89e] px-3 py-2 text-left font-bold whitespace-nowrap text-accent-foreground"
              >
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {cols.map((c) => (
            <tr key={c.name} className="even:bg-[rgba(196,184,158,0.12)]">
              <td className="border-b border-[#e8e2d6] px-3 py-1.5 font-bold whitespace-nowrap text-foreground">
                {c.name}
              </td>
              <td className="border-b border-[#e8e2d6] px-3 py-1.5 font-mono whitespace-nowrap text-[#11a89b]">
                {c.type}
              </td>
              <td className="border-b border-[#e8e2d6] px-3 py-1.5 font-medium whitespace-nowrap text-[#725d42]">
                {c.nullable ? "YES" : "NO"}
              </td>
              <td className="border-b border-[#e8e2d6] px-3 py-1.5 font-medium text-[#725d42]">
                {c.default === null ? <span className="text-[#c4b89e] italic">—</span> : c.default}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

// セグメント切替の 1 ボタン(DATA / STRUCTURE)。active はミント面。
function Seg({
  active,
  onClick,
  icon,
  children,
}: {
  active: boolean;
  onClick: () => void;
  icon: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        "flex min-h-9 items-center gap-1.5 rounded-xl px-3 py-1.5 text-xs font-bold tracking-wide outline-none transition-all duration-150 focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-1",
        active
          ? "bg-[#0CC0B5] text-[#FFF9E3] shadow-[0_2px_0_0_rgba(61,52,40,0.08)]"
          : "text-[#794f27] hover:text-[#11a89b]",
      )}
    >
      {icon}
      {children}
    </button>
  );
}

function Loading() {
  return (
    <div className="flex items-center gap-2 px-1 py-6 text-sm font-semibold text-muted-foreground">
      <div className="size-4 animate-spin rounded-full border-2 border-[#d4c9b4] border-t-primary" />
      読み込み中…
    </div>
  );
}

// 右ペインの空表示(テーブル未選択 / テーブルが 1 つも無い)。
function EmptyRight({ hasTables }: { hasTables: boolean }) {
  const { id = "" } = useParams();
  return (
    // 壁紙が透けて読みにくくならないよう、原典の dashed カード(薄クリーム面 +
    // 破線枠)を使う。中身は縦積みで中央寄せ。
    <Card
      type="dashed"
      className="min-h-[40vh] items-center justify-center gap-3 px-6 py-12 text-center"
    >
      <div className="grid size-14 place-items-center rounded-full bg-accent text-accent-foreground">
        <Database className="size-7" />
      </div>
      {hasTables ? (
        <p className="text-sm font-semibold text-muted-foreground">
          左の一覧からテーブルを選んでください。
        </p>
      ) : (
        <div className="flex flex-col items-center gap-2">
          <p className="text-base font-bold text-foreground">まだテーブルがありません</p>
          <p className="max-w-xs text-sm font-medium text-muted-foreground">
            SQL コンソールで <code className="font-mono">CREATE TABLE …</code>{" "}
            を実行するとここに並びます。
          </p>
          <Link
            to={`/databases/${id}/editor`}
            className="text-sm font-bold text-[#11a89b] underline-offset-2 hover:underline"
          >
            SQL コンソールを開く →
          </Link>
        </div>
      )}
    </Card>
  );
}
