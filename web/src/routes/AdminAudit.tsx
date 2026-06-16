import { useEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";

import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Divider } from "@/components/ui/divider";
import { Skeleton } from "@/components/ui/skeleton";
import { Title } from "@/components/ui/title";
import { useAuditLog } from "@/lib/admin";
import { cn } from "@/lib/utils";

// 監査ログ閲覧(owner 専用)。誰が・いつ・何をしたか。owner ゲートは <RequireOwner>(router)。
// 後端も owner + session を毎回検証。件数が増えうるので **TanStack Virtual で仮想リスト化**:
// 見える行だけ DOM に描き、末尾に近づくと自動で次頁を取る(キーセット分頁の「もっと読む」を
// スクロールに置き換え)。行高は一定にするため詳細は 1 行省略(全文は title 属性で出す)。

const FILTERS: { key: string; label: string }[] = [
  { key: "", label: "すべて" },
  { key: "owner.", label: "管理者代理" },
  { key: "service.", label: "サービス" },
  { key: "db.", label: "データベース" },
  { key: "volume.", label: "ボリューム" },
  { key: "disk.", label: "ディスク警告" },
];

// ヘッダと各行で共有するグリッド列(時刻 / アクション / 操作者 / 対象ユーザ / 詳細)。
const COLS =
  "grid grid-cols-[150px_minmax(140px,1fr)_130px_130px_minmax(180px,2fr)] items-center gap-4 px-4";
// 仮想化のための一定行高(px)。詳細を 1 行省略にして高さを揃える。
const ROW_HEIGHT = 52;

function detailText(detail: unknown): string {
  if (detail == null) return "";
  if (typeof detail === "object" && Object.keys(detail).length === 0) return "";
  return JSON.stringify(detail);
}

export default function AdminAudit() {
  const [action, setAction] = useState("");
  const { data, isPending, error, fetchNextPage, hasNextPage, isFetchingNextPage } =
    useAuditLog(action);
  const rows = data?.pages.flat() ?? [];

  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 10,
  });
  const virtualItems = virtualizer.getVirtualItems();

  // 末尾付近まで来たら次頁を自動取得(「もっと読む」をスクロールに置換)。
  useEffect(() => {
    const last = virtualItems[virtualItems.length - 1];
    if (last && last.index >= rows.length - 1 && hasNextPage && !isFetchingNextPage) {
      fetchNextPage();
    }
  }, [virtualItems, rows.length, hasNextPage, isFetchingNextPage, fetchNextPage]);

  const skeletonKeys = ["a", "b", "c", "d", "e", "f", "g", "h"];

  return (
    <PageContainer>
      <div className="flex flex-col gap-7">
        <PageMeta title="監査ログ" />

        <Title size="large" color="purple">
          監査ログ
        </Title>

        <Divider type="line-brown" />

        <p className="max-w-2xl text-sm font-medium text-muted-foreground">
          誰が・いつ・何をしたかの記録(管理者の代理操作・作成 / 削除・rotate・ディスク警告 など)。
          下までスクロールすると古い記録を自動で読み込みます。
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

        {!error && (isPending || rows.length > 0) && (
          <Card>
            <CardContent className="p-0">
              {/* ヘッダはスクロール枠の**外**に置く(仮想化のオフセット計算に header 高が
                  混ざらないように)。列はスクロール内の行と同じ COLS グリッドで揃える。 */}
              <div
                className={cn(
                  COLS,
                  "border-b-2 border-[rgba(61,52,40,0.08)] py-3 text-xs font-bold text-muted-foreground",
                )}
              >
                <div>時刻</div>
                <div>アクション</div>
                <div>操作者</div>
                <div>対象ユーザ</div>
                <div>詳細</div>
              </div>

              <div ref={scrollRef} className="max-h-[60vh] overflow-auto [scrollbar-gutter:stable]">
                {isPending ? (
                  // 初回読み込み:Skeleton 行(仮想化なし)。
                  <div>
                    {skeletonKeys.map((k) => (
                      <div
                        key={k}
                        className={cn(COLS, "border-b border-[rgba(61,52,40,0.06)] py-3")}
                        style={{ height: ROW_HEIGHT }}
                      >
                        <Skeleton className="h-4 w-28" />
                        <Skeleton className="h-4 w-24" />
                        <Skeleton className="h-4 w-20" />
                        <Skeleton className="h-4 w-20" />
                        <Skeleton className="h-4 w-full" />
                      </div>
                    ))}
                  </div>
                ) : (
                  // 仮想リスト本体:総高のスペーサ内に、見える行だけ絶対配置。
                  <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
                    {virtualItems.map((vi) => {
                      const e = rows[vi.index];
                      const detail = detailText(e.detail);
                      return (
                        <div
                          key={e.id}
                          className={cn(COLS, "border-b border-[rgba(61,52,40,0.06)] py-3 text-sm")}
                          style={{
                            position: "absolute",
                            top: 0,
                            left: 0,
                            width: "100%",
                            height: vi.size,
                            transform: `translateY(${vi.start}px)`,
                          }}
                        >
                          <div className="truncate text-xs font-medium text-muted-foreground">
                            {new Date(e.created_at).toLocaleString("ja-JP")}
                          </div>
                          <div className="truncate font-mono text-xs font-bold text-[#0b9c93]">
                            {e.action}
                          </div>
                          <div className="truncate font-semibold text-foreground">
                            {e.actor_name ?? (
                              <span className="text-muted-foreground">システム</span>
                            )}
                          </div>
                          <div className="truncate font-medium text-foreground">
                            {e.target_user_name ?? <span className="text-muted-foreground">—</span>}
                          </div>
                          <div
                            className="truncate font-mono text-xs text-muted-foreground"
                            title={detail}
                          >
                            {detail}
                          </div>
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            </CardContent>
          </Card>
        )}

        {/* 末尾の追加読み込み中インジケータ。 */}
        {isFetchingNextPage && (
          <p className="text-center text-sm font-medium text-muted-foreground">読み込み中…</p>
        )}

        {/* 読み込み完了かつ 0 件のときだけ空表示。 */}
        {!error && !isPending && rows.length === 0 && (
          <Card type="dashed">
            <CardContent className="px-6 py-12 text-center">
              <p className="text-sm font-medium text-muted-foreground">記録がありません。</p>
            </CardContent>
          </Card>
        )}
      </div>
    </PageContainer>
  );
}
