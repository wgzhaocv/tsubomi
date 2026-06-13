import { useState } from "react";
import { Play } from "lucide-react";
import { useParams } from "react-router";

import { ResultTable } from "@/components/query-result";
import { Button } from "@/components/ui/button";
import { useRunQuery } from "@/lib/databases";

// SQL コンソール(独立ページ)。当該 DB 自身の human 資格でサーバ側が実行する
// (statement_timeout 10s + サーバ側 15s の硬い上限)。任意 SQL を流せる。

export default function DatabaseEditor() {
  const { id = "" } = useParams();
  const runQuery = useRunQuery(id);
  const [sql, setSql] = useState("");

  const run = () => {
    if (sql.trim()) runQuery.mutate(sql);
  };

  return (
    <div className="flex flex-col gap-3">
      <textarea
        value={sql}
        onChange={(e) => setSql(e.target.value)}
        onKeyDown={(e) => {
          // Cmd/Ctrl+Enter で実行(エディタの定番)。
          if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
            e.preventDefault();
            run();
          }
        }}
        placeholder="SELECT * FROM ..."
        spellCheck={false}
        rows={10}
        className="w-full resize-y rounded-2xl border-2 border-[#c4b89e] bg-[rgb(247,243,223)] px-4 py-3 font-['SF_Mono','Fira_Code',Consolas,monospace] text-sm text-[#725d42] outline-none placeholder:text-[#c4b89e] focus-visible:[outline:2px_solid_#f5c31c] focus-visible:outline-offset-2"
      />
      <div className="flex flex-wrap items-center gap-3">
        <Button
          type="primary"
          icon={<Play className="size-4" />}
          loading={runQuery.isPending}
          disabled={!sql.trim()}
          onClick={run}
        >
          実行
        </Button>
        <span className="text-xs font-medium text-muted-foreground">
          ⌘/Ctrl+Enter で実行。このデータベース自身の資格情報で動きます(statement_timeout 10s)。
        </span>
      </div>

      {runQuery.error && (
        <pre className="overflow-auto rounded-2xl border-2 border-[#e05a5a] bg-[rgba(224,90,90,0.08)] px-4 py-3 text-sm font-semibold whitespace-pre-wrap text-[#c94444]">
          {runQuery.error.message}
        </pre>
      )}

      {runQuery.data && <ResultTable result={runQuery.data} />}
    </div>
  );
}
