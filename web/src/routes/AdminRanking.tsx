import { useMemo, useState } from "react";
import { Square, Trash2 } from "lucide-react";

import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Divider } from "@/components/ui/divider";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { Title } from "@/components/ui/title";
import {
  type AdminAction,
  type AdminResourceRow,
  formatUsageByKind,
  KIND_LABEL,
  useAdminAction,
  useAdminRanking,
} from "@/lib/admin";
import { useMeQuery } from "@/lib/auth";

// 使用量ランキング(owner 専用)。匿名行(真名 + 匿名番号 + 使用量)を降順で。
// 種別フィルタはセグメント(全て / サービス / DB / ボリューム)= 画面側で絞る
// (全件を 1 回取得し、タブ切替で再取得しない)。owner ゲートは <RequireOwner>(router)。
// 「最後の砦」:owner は他人の資源を停止 / 削除できる(後端が owner + session + メール検証コードを
// 毎回検証)。押下 → owner 自身にコードがメールされ、コード入力モーダルで確定する二段確認。

const FILTERS: { key: string; label: string }[] = [
  { key: "all", label: "すべて" },
  { key: "service", label: "サービス" },
  { key: "database", label: "データベース" },
  { key: "volume", label: "ボリューム" },
  { key: "cache", label: "キャッシュ" },
];

const ACTION_LABEL: Record<AdminAction, string> = { stop: "停止", delete: "削除" };

function usageText(row: AdminResourceRow): string {
  return formatUsageByKind(row.kind, row.usage_bytes);
}

// 「使用量」は種別で意味が違う(service=稼働中メモリ / database=ストレージ / volume=ディスク /
// cache=キー数)。1 列に混ぜて降順にしているので、行ごとに何の指標かを併記して誤読を防ぐ。
const USAGE_METRIC: Record<string, string> = {
  service: "メモリ",
  database: "ストレージ",
  volume: "ディスク",
  cache: "キー数",
};

function usageMetric(row: AdminResourceRow): string {
  return USAGE_METRIC[row.kind] ?? "使用量";
}

function serviceState(row: AdminResourceRow): string {
  if (row.kind !== "service" || row.running == null) return "—";
  return row.running ? "稼働中" : "停止中";
}

function cpuText(row: AdminResourceRow): string {
  if (row.kind !== "service" || row.cpu_pct == null) return "—";
  return `${row.cpu_pct.toFixed(1)}%`;
}

export default function AdminRanking() {
  const [kind, setKind] = useState("all");
  const { data: allRows, isPending, error } = useAdminRanking();
  const action = useAdminAction();
  // 危険操作(停止 / 削除)は owner のみ。viewer は表を見られるが操作列は出さない
  // (表示制御は UX — 後端の actions は owner + session + メール検証を毎回確認)。
  const { data: me } = useMeQuery();
  const isOwner = me?.role === "owner";

  // 二段確認のモーダル状態(対象行 + 操作)とコード入力。
  const [pending, setPending] = useState<{ row: AdminResourceRow; act: AdminAction } | null>(null);
  const [code, setCode] = useState("");

  // 種別フィルタは画面側(サーバは全件を使用量降順で返す。順序は保たれる)。
  const rows = useMemo(
    () => (kind === "all" ? allRows : allRows?.filter((r) => r.kind === kind)),
    [allRows, kind],
  );

  // 1 段目:コードを請求(サーバが owner にメール送信)→ 成功でモーダルを開く。
  const start = (row: AdminResourceRow, act: AdminAction) => {
    action.reset();
    action.mutate(
      { id: row.resource_id, action: act },
      {
        onSuccess: (data) => {
          if (data.code_required) {
            setPending({ row, act });
            setCode("");
          }
        },
      },
    );
  };

  // モーダルを閉じる(対象とコード入力の両方をクリア)。
  const closeModal = () => {
    setPending(null);
    setCode("");
  };

  // 2 段目:コードを入れて確定 → 実行。
  const confirm = () => {
    if (!pending || !code.trim() || action.isPending) return;
    action.mutate(
      { id: pending.row.resource_id, action: pending.act, code: code.trim() },
      {
        onSuccess: (data) => {
          if (!data.code_required) closeModal();
        },
      },
    );
  };

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
                    {isOwner && <th className="px-4 py-3 text-right">操作</th>}
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
                      <td className="px-4 py-3 text-right">
                        <div className="font-mono font-bold text-foreground">{usageText(row)}</div>
                        <div className="text-xs font-medium text-muted-foreground">
                          {usageMetric(row)}
                        </div>
                      </td>
                      <td className="px-4 py-3 text-right font-mono text-muted-foreground">
                        {cpuText(row)}
                      </td>
                      <td className="px-4 py-3 text-right text-xs font-semibold text-muted-foreground">
                        {serviceState(row)}
                      </td>
                      {isOwner && (
                        <td className="px-4 py-3">
                          <div className="flex justify-end gap-2">
                            {row.kind === "service" && row.running && (
                              <Button
                                type="default"
                                size="small"
                                icon={<Square className="size-3.5" />}
                                onClick={() => start(row, "stop")}
                              >
                                停止
                              </Button>
                            )}
                            <Button
                              type="default"
                              size="small"
                              danger
                              icon={<Trash2 className="size-3.5" />}
                              onClick={() => start(row, "delete")}
                            >
                              削除
                            </Button>
                          </div>
                        </td>
                      )}
                    </tr>
                  ))}
                </tbody>
              </table>
            </CardContent>
          </Card>
        )}

        {/* 「最後の砦」の起点でエラーが出た場合(コード請求の失敗など)。 */}
        {action.error && !pending && (
          <p className="text-sm font-semibold text-[#e05a5a]">{action.error.message}</p>
        )}

        {/* 二段確認:owner のメールに届いたコードを入力。 */}
        <Modal
          open={pending !== null}
          title={pending ? `${ACTION_LABEL[pending.act]}の確認` : ""}
          typewriter={false}
          width={460}
          onClose={closeModal}
          footer={
            <>
              <Button type="text" onClick={closeModal}>
                キャンセル
              </Button>
              <Button type="primary" danger loading={action.isPending} onClick={confirm}>
                {pending ? ACTION_LABEL[pending.act] : ""}する
              </Button>
            </>
          }
        >
          {pending && (
            <form
              onSubmit={(ev) => {
                ev.preventDefault();
                confirm();
              }}
              className="flex w-full flex-col gap-3"
            >
              <p className="text-sm font-medium text-foreground">
                <strong className="font-mono text-[#0b9c93]">{pending.row.anon_label}</strong>(
                {pending.row.owner_name})を <strong>{ACTION_LABEL[pending.act]}</strong> します。
                確認コードをあなたのメール宛に送信しました。
              </p>
              <Input
                label="確認コード(6 桁)"
                placeholder="123456"
                value={code}
                autoFocus
                inputMode="numeric"
                onChange={(ev) => setCode(ev.target.value)}
                description="管理者のメールに届いた 6 桁のコードを入力してください(有効期限 10 分)。"
              />
              {action.error && (
                <p className="text-sm font-semibold text-[#e05a5a]">{action.error.message}</p>
              )}
            </form>
          )}
        </Modal>
      </div>
    </PageContainer>
  );
}
