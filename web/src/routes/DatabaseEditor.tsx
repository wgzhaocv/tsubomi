import { useRef, useState } from "react";
import { Play, WandSparkles } from "lucide-react";
import { useParams } from "react-router";

import { ResultTable } from "@/components/query-result";
import { Button } from "@/components/ui/button";
import type { QueryResponse } from "@/lib/databases";
import { useRunQuery } from "@/lib/databases";
import { useEditorStore } from "@/lib/store/editor";

// SQL コンソール(独立ページ)。当該 DB 自身の human 資格でサーバ側が実行する
// (statement_timeout 10s + サーバ側 15s の硬い上限)。任意 SQL を流せる。
// SQL 草稿と直近結果は zustand に置く(画面遷移で消えない。SQL は localStorage 永続)。

// このマシンの修飾キー表示。実行判定は metaKey||ctrlKey の両対応なので、ここは
// ラベルだけの問題(Mac は ⌘、それ以外は Ctrl)。
const isMac =
  typeof navigator !== "undefined" &&
  /Mac|iP(hone|ad|od)/.test(navigator.platform || navigator.userAgent);
const MOD_KEY = isMac ? "⌘" : "Ctrl";

export default function DatabaseEditor() {
  const { id = "" } = useParams();
  const runQuery = useRunQuery(id);

  // SQL 草稿・直近結果・高さはすべて zustand(遷移で消えない)。
  const sql = useEditorStore((s) => s.sqlByDb[id] ?? "");
  const setSql = useEditorStore((s) => s.setSql);
  const stored = useEditorStore((s) => s.resultByDb[id]);
  const setResult = useEditorStore((s) => s.setResult);
  const height = useEditorStore((s) => s.height);
  const setHeight = useEditorStore((s) => s.setHeight);

  // 選択範囲があるか(ボタン文言と実行範囲の判定に使う)。
  const [hasSelection, setHasSelection] = useState(false);
  const taRef = useRef<HTMLTextAreaElement>(null);

  // 実行する SQL を決める:選択範囲があればその部分だけ、無ければ全文。
  const run = () => {
    const el = taRef.current;
    const selected =
      el && el.selectionStart !== el.selectionEnd
        ? el.value.slice(el.selectionStart, el.selectionEnd)
        : sql;
    if (!selected.trim()) return;
    runQuery.mutate(selected, {
      onSuccess: (resp) => setResult(id, { ok: resp }),
      onError: (e) => setResult(id, { error: e.message }),
    });
  };

  // SQL 整形(best-effort)。sql-formatter は重い(~260KB)ので初回整形時に動的 import。
  // パースできない SQL はそのまま据え置く(実行時にサーバがエラーを返す)。整形は全文。
  const formatSql = async () => {
    if (!sql.trim()) return;
    try {
      const { format } = await import("sql-formatter");
      setSql(id, format(sql, { language: "postgresql" }));
    } catch {
      // 不正な SQL は整形しない。
    }
  };

  // ドラッグバーで高さ調整。pointer capture せず window で move/up を拾う。
  const onDragStart = (e: React.PointerEvent) => {
    e.preventDefault();
    const startY = e.clientY;
    const startH = taRef.current?.offsetHeight ?? height;
    const onMove = (ev: PointerEvent) => setHeight(startH + (ev.clientY - startY));
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };

  return (
    <div className="flex flex-col gap-3">
      {/* textarea + 下端のドラッグバーを 1 つの枠に収める。focus 枠は枠側に出す。 */}
      <div className="flex flex-col overflow-hidden rounded-2xl border-2 border-[#c4b89e] bg-[rgb(247,243,223)] focus-within:[outline:2px_solid_#f5c31c] focus-within:outline-offset-2">
        <textarea
          ref={taRef}
          value={sql}
          onChange={(e) => setSql(id, e.target.value)}
          onSelect={(e) => {
            const el = e.currentTarget;
            setHasSelection(el.selectionStart !== el.selectionEnd);
          }}
          onKeyDown={(e) => {
            // Cmd/Ctrl+Enter で実行(選択中ならその範囲だけ)。
            if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
              e.preventDefault();
              run();
            }
          }}
          placeholder="SELECT * FROM ..."
          spellCheck={false}
          style={{ height }}
          className="w-full resize-none bg-transparent px-4 py-3 font-['SF_Mono','Fira_Code',Consolas,monospace] text-sm text-[#725d42] outline-none placeholder:text-[#c4b89e]"
        />
        {/* ドラッグバー:上下ドラッグで高さ変更。a11y で上下キーにも対応。 */}
        <div
          role="separator"
          aria-orientation="horizontal"
          aria-label="エディタの高さを調整"
          tabIndex={0}
          onPointerDown={onDragStart}
          onKeyDown={(e) => {
            if (e.key === "ArrowUp") {
              e.preventDefault();
              setHeight(height - 24);
            } else if (e.key === "ArrowDown") {
              e.preventDefault();
              setHeight(height + 24);
            }
          }}
          className="flex h-3.5 shrink-0 cursor-ns-resize items-center justify-center border-t-2 border-[#e8dcc8] bg-[rgba(196,184,158,0.15)] outline-none transition-colors hover:bg-[rgba(196,184,158,0.35)] focus-visible:[outline:2px_solid_#19c8b9] focus-visible:[outline-offset:-2px]"
        >
          <div className="h-1 w-9 rounded-full bg-[#c4b89e]" />
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-3">
        <Button
          type="primary"
          icon={<Play className="size-4" />}
          loading={runQuery.isPending}
          disabled={!sql.trim()}
          onClick={run}
        >
          {hasSelection ? "選択を実行" : "実行"}
        </Button>
        <Button
          type="default"
          icon={<WandSparkles className="size-4" />}
          disabled={!sql.trim()}
          onClick={formatSql}
        >
          整形
        </Button>
        <span className="text-xs font-medium text-muted-foreground">
          {MOD_KEY}+Enter で実行(選択中ならその範囲だけ)。このデータベース自身の資格情報で 動きます(
          <code className="rounded bg-[rgba(196,184,158,0.2)] px-1 py-0.5 font-['SF_Mono','Fira_Code',Consolas,monospace] text-[11px] text-[#725d42]">
            statement_timeout 10s
          </code>
          )。
        </span>
      </div>

      {stored && "error" in stored && (
        <pre className="overflow-auto rounded-2xl border-2 border-[#e05a5a] bg-[rgba(224,90,90,0.08)] px-4 py-3 text-sm font-semibold whitespace-pre-wrap text-[#c94444]">
          {stored.error}
        </pre>
      )}
      {stored && "ok" in stored && <ResultsView resp={stored.ok} />}
    </div>
  );
}

// 複数文の結果を、文ごとに分けて表示する(混ぜない)。SELECT は表、それ以外は
// 「OK · N 行に影響」。結果が複数あるときは見出しを付ける。
function ResultsView({ resp }: { resp: QueryResponse }) {
  if (resp.results.length === 0) {
    return <p className="text-sm font-semibold text-[#11a89b]">OK。</p>;
  }
  const multi = resp.results.length > 1;
  return (
    <div className="flex flex-col gap-4">
      {resp.results.map((set, i) => (
        <div key={i} className="flex flex-col gap-1.5">
          {multi && (
            <p className="text-xs font-bold text-muted-foreground">
              結果 {i + 1} / {resp.results.length}
            </p>
          )}
          {set.columns.length > 0 ? (
            <ResultTable result={set} />
          ) : (
            <p className="text-sm font-semibold text-[#11a89b]">
              OK{set.rows_affected > 0 ? ` · ${set.rows_affected} 行に影響` : ""}
            </p>
          )}
        </div>
      ))}
    </div>
  );
}
