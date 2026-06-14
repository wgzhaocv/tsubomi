import { useMemo, useState } from "react";

import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Divider } from "@/components/ui/divider";
import { Title } from "@/components/ui/title";
import { type AdminResourceRow, KIND_LABEL, useAdminRanking } from "@/lib/admin";
import { formatBytes } from "@/lib/volumes";

// 使用量ランキング(owner 専用)。匿名行(真名 + 匿名番号 + 使用量)を降順で。
// 種別フィルタはセグメント(全て / サービス / DB / ボリューム)= 画面側で絞る
// (全件を 1 回取得し、タブ切替で再取得しない)。owner ゲートは <RequireOwner>(router)。

const FILTERS: { key: string; label: string }[] = [
  { key: "all", label: "すべて" },
  { key: "service", label: "サービス" },
  { key: "database", label: "データベース" },
  { key: "volume", label: "ボリューム" },
];

function usageText(row: AdminResourceRow): string {
  return row.usage_bytes == null ? "—" : formatBytes(row.usage_bytes);
}

function serviceState(row: AdminResourceRow): string {
  if (row.kind !== "service") return "—";
  if (row.running == null) return "—";
  return row.running ? "稼働中" : "停止中";
}

function cpuText(row: AdminResourceRow): string {
  if (row.kind !== "service" || row.cpu_pct == null) return "—";
  return `${row.cpu_pct.toFixed(1)}%`;
}

export default function AdminRanking() {
  const [kind, setKind] = useState("all");
  const { data: allRows, isPending, error } = useAdminRanking();

  // 種別フィルタは画面側(サーバは全件を使用量降順で返す。順序は保たれる)。
  const rows = useMemo(
    () => (kind === "all" ? allRows : allRows?.filter((r) => r.kind === kind)),
    [allRows, kind],
  );

  return (
    <PageContainer>
      <div className="flex flex-col gap-7">
        <PageMeta title="使用量ランキング" />

        <Title size="large" color="purple">
          使用量ランキング
        </Title>

        <Divider type="line-brown" />

        {/* 種別フィルタ(セグメント)。 */}
        <div className="flex flex-wrap gap-2">
          {FILTERS.map((f) => (
            <Button
              key={f.key}
              type={kind === f.key ? "primary" : "default"}
              size="small"
              onClick={() => setKind(f.key)}
            >
              {f.label}
            </Button>
          ))}
        </div>

        {error && (
          <p className="text-sm font-semibold text-[#e05a5a]">
            読み込みに失敗しました:{error.message}
          </p>
        )}

        {isPending && !rows && (
          <p className="text-sm font-medium text-muted-foreground">読み込み中…</p>
        )}

        {rows && rows.length === 0 && (
          <Card type="dashed">
            <CardContent className="px-6 py-12 text-center">
              <p className="text-sm font-medium text-muted-foreground">
                該当する資源がありません。
              </p>
            </CardContent>
          </Card>
        )}

        {rows && rows.length > 0 && (
          <Card>
            <CardContent className="overflow-x-auto p-0">
              <table className="w-full border-collapse text-sm">
                <thead>
                  <tr className="border-b-2 border-[rgba(61,52,40,0.08)] text-left text-xs font-bold text-muted-foreground">
                    <th className="px-4 py-3">利用者</th>
                    <th className="px-4 py-3">資源</th>
                    <th className="px-4 py-3 text-right">使用量</th>
                    <th className="px-4 py-3 text-right">CPU</th>
                    <th className="px-4 py-3 text-right">状態</th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((row) => (
                    <tr
                      key={row.resource_id}
                      className="border-b border-[rgba(61,52,40,0.06)] last:border-0"
                    >
                      <td className="px-4 py-3 font-semibold text-foreground">{row.owner_name}</td>
                      <td className="px-4 py-3">
                        <span className="font-mono text-xs font-bold text-[#0b9c93]">
                          {row.anon_label}
                        </span>
                        <span className="ml-2 text-xs font-medium text-muted-foreground">
                          {KIND_LABEL[row.kind] ?? row.kind}
                        </span>
                      </td>
                      <td className="px-4 py-3 text-right font-mono font-bold text-foreground">
                        {usageText(row)}
                      </td>
                      <td className="px-4 py-3 text-right font-mono text-muted-foreground">
                        {cpuText(row)}
                      </td>
                      <td className="px-4 py-3 text-right text-xs font-semibold text-muted-foreground">
                        {serviceState(row)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </CardContent>
          </Card>
        )}
      </div>
    </PageContainer>
  );
}
