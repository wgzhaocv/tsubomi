import { useState } from "react";
import { RotateCcw, Trash2 } from "lucide-react";

import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Divider } from "@/components/ui/divider";
import { Modal } from "@/components/ui/modal";
import { Title } from "@/components/ui/title";
import { type TrashItem, usePurge, useRestore, useTrash } from "@/lib/trash";

// ゴミ箱:4 種リソース共通の一覧 + 復元 + 完全削除(名前確認なしだが確認モーダルあり)。
// 削除は 3 日の猶予 → 期限到来で reconcile が自動 purge。ここからは即時復元 / 即時完全削除。

const KIND_JA: Record<string, string> = {
  service: "サービス",
  database: "データベース",
  cache: "キャッシュ",
  volume: "ボリューム",
};

// 自動削除までの残り日数(purge_after - now)。
function daysLeft(purgeAfter: string | null): string {
  if (!purgeAfter) return "—";
  const ms = new Date(purgeAfter).getTime() - Date.now();
  const days = Math.ceil(ms / 86_400_000);
  return days > 0 ? `あと ${days} 日` : "まもなく";
}

export default function Trash() {
  const { data: items, isPending, error } = useTrash();
  const restore = useRestore();
  const purge = usePurge();
  const [purgeTarget, setPurgeTarget] = useState<TrashItem | null>(null);

  const opError = restore.error || purge.error;

  return (
    <PageContainer>
      <div className="flex flex-col gap-7">
        <PageMeta title="ゴミ箱" />

        <header className="flex flex-wrap items-center justify-between gap-4">
          <Title size="large" color="brown">
            ゴミ箱
          </Title>
        </header>

        <Divider type="line-brown" />

        {error && (
          <p className="text-sm font-semibold text-[#e05a5a]">
            読み込みに失敗しました:{error.message}
          </p>
        )}
        {opError && <p className="text-sm font-semibold text-[#e05a5a]">{opError.message}</p>}

        {!isPending && items && items.length === 0 && (
          <Card type="dashed">
            <CardContent className="flex flex-col items-center gap-4 px-6 py-12 text-center">
              <div className="grid size-16 place-items-center rounded-full bg-accent text-accent-foreground">
                <Trash2 className="size-8" />
              </div>
              <div className="flex flex-col gap-1.5">
                <p className="text-lg font-bold text-foreground">ゴミ箱は空です</p>
                <p className="max-w-md text-sm font-medium text-muted-foreground">
                  削除したリソースは 3
                  日間ここに保管され、復元できます。期限を過ぎると自動的に完全削除されます。
                </p>
              </div>
            </CardContent>
          </Card>
        )}

        {items && items.length > 0 && (
          <ul className="flex flex-col gap-3">
            {items.map((it) => (
              <li key={it.id}>
                <Card className="flex-row items-center justify-between gap-4 py-4">
                  <CardContent className="flex min-w-0 items-center gap-3.5">
                    <span className="shrink-0 rounded-full bg-accent px-3 py-1 text-xs font-bold text-accent-foreground">
                      {KIND_JA[it.kind] ?? it.kind}
                    </span>
                    <div className="flex min-w-0 flex-col">
                      <span className="truncate text-base font-bold text-foreground">
                        {it.display_name}
                      </span>
                      <span className="truncate text-xs font-medium text-muted-foreground">
                        削除 {new Date(it.deleted_at).toLocaleDateString("ja-JP")} · 自動削除{" "}
                        {daysLeft(it.purge_after)}
                      </span>
                    </div>
                  </CardContent>
                  <div className="flex shrink-0 gap-2">
                    <Button
                      type="default"
                      size="small"
                      icon={<RotateCcw className="size-4" />}
                      loading={restore.isPending && restore.variables === it.id}
                      onClick={() => restore.mutate(it.id)}
                    >
                      復元
                    </Button>
                    <Button
                      type="default"
                      size="small"
                      danger
                      icon={<Trash2 className="size-4" />}
                      onClick={() => setPurgeTarget(it)}
                    >
                      完全に削除
                    </Button>
                  </div>
                </Card>
              </li>
            ))}
          </ul>
        )}

        {/* 完全削除の確認(元に戻せない) */}
        <Modal
          open={purgeTarget !== null}
          title="完全に削除"
          typewriter={false}
          width={460}
          onClose={() => setPurgeTarget(null)}
          footer={
            <>
              <Button type="text" onClick={() => setPurgeTarget(null)}>
                キャンセル
              </Button>
              <Button
                type="primary"
                danger
                loading={purge.isPending}
                onClick={() => {
                  if (!purgeTarget) return;
                  purge.mutate(purgeTarget.id, { onSuccess: () => setPurgeTarget(null) });
                }}
              >
                完全に削除する
              </Button>
            </>
          }
        >
          <p>
            <strong>{purgeTarget?.display_name}</strong> を完全に削除します。
            <strong>元に戻せません。</strong>続けますか?
          </p>
        </Modal>
      </div>
    </PageContainer>
  );
}
