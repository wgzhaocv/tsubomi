import * as React from "react";

// 状態グリッドの 1 セル(ラベル + 値)。クリーム面、罫線は親 <dl> の gap-px が描く。
// database / cache の概要の「状態」グリッドが共有する(<dl className="grid ... gap-px"> の中に並べる)。
export function Stat({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1 bg-card px-4 py-3">
      <dt className="text-xs font-semibold text-muted-foreground">{label}</dt>
      <dd className="text-sm font-bold text-foreground">{children}</dd>
    </div>
  );
}
