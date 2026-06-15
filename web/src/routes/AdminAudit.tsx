import { useState } from "react";

import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Divider } from "@/components/ui/divider";
import { Title } from "@/components/ui/title";
import { type AuditEntry, useAuditLog } from "@/lib/admin";

// 監査ログ閲覧(owner 専用)。誰が・いつ・何をしたか(owner 代理操作 / 作成削除 / rotate /
// ディスク警告 …)。owner ゲートは <RequireOwner>(router)。後端も owner + session を毎回検証。
// 「監査 = 可視性のもう半分」。

// action の前方一致フィルタのプリセット(サーバが LIKE prefix||'%' で絞る)。
const FILTERS: { key: string; label: string }[] = [
  { key: "", label: "すべて" },
  { key: "owner.", label: "owner 代理" },
  { key: "service.", label: "サービス" },
  { key: "db.", label: "データベース" },
  { key: "volume.", label: "ボリューム" },
  { key: "disk.", label: "ディスク警告" },
];

function detailText(detail: unknown): string {
  if (detail == null) return "";
  if (typeof detail === "object" && Object.keys(detail).length === 0) return "";
  return JSON.stringify(detail);
}

function Row({ e }: { e: AuditEntry }) {
  return (
    <tr className="border-b border-[rgba(61,52,40,0.06)] last:border-0 align-top">
      <td className="px-4 py-3 whitespace-nowrap text-xs font-medium text-muted-foreground">
        {new Date(e.created_at).toLocaleString("ja-JP")}
      </td>
      <td className="px-4 py-3">
        <span className="font-mono text-xs font-bold text-[#0b9c93]">{e.action}</span>
      </td>
      <td className="px-4 py-3 text-sm font-semibold text-foreground">
        {e.actor_name ?? <span className="text-muted-foreground">システム</span>}
      </td>
      <td className="px-4 py-3 text-sm font-medium text-foreground">
        {e.target_user_name ?? <span className="text-muted-foreground">—</span>}
      </td>
      <td className="px-4 py-3 font-mono text-xs break-all text-muted-foreground">
        {detailText(e.detail)}
      </td>
    </tr>
  );
}

export default function AdminAudit() {
  const [action, setAction] = useState("");
  const { data, isPending, error, fetchNextPage, hasNextPage, isFetchingNextPage } =
    useAuditLog(action);

  const rows = data?.pages.flat() ?? [];

  return (
    <PageContainer>
      <div className="flex flex-col gap-7">
        <PageMeta title="監査ログ" />

        <Title size="large" color="purple">
          監査ログ
        </Title>

        <Divider type="line-brown" />

        <p className="max-w-2xl text-sm font-medium text-muted-foreground">
          誰が・いつ・何をしたかの記録(owner の代理操作・作成 / 削除・rotate・ディスク警告 など)。
        </p>

        {/* action の前方一致フィルタ。 */}
        <div className="flex flex-wrap gap-2">
          {FILTERS.map((f) => (
            <Button
              key={f.key}
              type={action === f.key ? "primary" : "default"}
              size="small"
              onClick={() => setAction(f.key)}
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

        {isPending && <p className="text-sm font-medium text-muted-foreground">読み込み中…</p>}

        {!isPending && rows.length === 0 && (
          <Card type="dashed">
            <CardContent className="px-6 py-12 text-center">
              <p className="text-sm font-medium text-muted-foreground">記録がありません。</p>
            </CardContent>
          </Card>
        )}

        {rows.length > 0 && (
          <>
            <Card>
              <CardContent className="overflow-x-auto p-0">
                <table className="w-full border-collapse text-sm">
                  <thead>
                    <tr className="border-b-2 border-[rgba(61,52,40,0.08)] text-left text-xs font-bold text-muted-foreground">
                      <th className="px-4 py-3">時刻</th>
                      <th className="px-4 py-3">アクション</th>
                      <th className="px-4 py-3">操作者</th>
                      <th className="px-4 py-3">対象ユーザ</th>
                      <th className="px-4 py-3">詳細</th>
                    </tr>
                  </thead>
                  <tbody>
                    {rows.map((e) => (
                      <Row key={e.id} e={e} />
                    ))}
                  </tbody>
                </table>
              </CardContent>
            </Card>

            {hasNextPage && (
              <div className="flex justify-center">
                <Button type="default" loading={isFetchingNextPage} onClick={() => fetchNextPage()}>
                  もっと読む
                </Button>
              </div>
            )}
          </>
        )}
      </div>
    </PageContainer>
  );
}
