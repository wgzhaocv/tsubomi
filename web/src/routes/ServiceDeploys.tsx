import { useParams } from "react-router";

import { Button } from "@/components/ui/button";
import {
  type Deploy,
  deployStatusLabel,
  shortDigest,
  useRollbackService,
  useService,
  useServiceDeploys,
} from "@/lib/services";

// デプロイ履歴。succeeded 行から同じ digest を再起動できる(rollback = 再 build なし)。
// **今動いている 1 行だけ**文言を変える — 自分へ「戻す」は意味不明だが、**同じイメージの
// 再デプロイは必要な操作**(rotate 後・env / 注入の変更後は起動時解決をやり直す)なので消さない。
export default function ServiceDeploys() {
  const { id = "" } = useParams();
  const { data: deploys, isPending, error } = useServiceDeploys(id);
  const { data: svc } = useService(id);
  const rollback = useRollbackService(id);
  // 今 serving している **1 件**の deploy id =「digest が一致する最初の成功行」。
  // digest だけで判定すると、同じイメージを再デプロイした履歴が複数在るとき**全部が「稼働中」に
  // 見えてしまう**(実際に走っているのは 1 つ)。
  // 門は `phase` ではなく **`desired_state`**(ユーザの意図):phase で門を掛けると、**新しい
  // デプロイが失敗した直後**(旧版は start-first なので serving を続けている)に phase=failed で
  // バッジが恒久的に消える(codex review 2026-07-26)。停止中は desired_state=stopped なので
  // 「稼働中」と嘘をつかない。
  // 注:サーバ側の真源(`latest_succeeded_deploy`)は digest を見ず「最後に成功した行」を選ぶ。
  // commit_success が status と image_digest を同一 tx で書くので結果は一致する = 近似で足りる。
  const servingDeployId =
    svc?.desired_state === "running"
      ? deploys?.find((d) => d.status === "succeeded" && d.image_digest === svc.image_digest)?.id
      : undefined;

  return (
    <div className="flex flex-col gap-4">
      <h2 className="text-lg font-bold text-foreground">デプロイ履歴</h2>

      {error && <p className="text-sm font-semibold text-[#e05a5a]">{error.message}</p>}
      {rollback.error && (
        <p className="text-sm font-semibold text-[#e05a5a]">{rollback.error.message}</p>
      )}

      {!isPending && deploys && deploys.length === 0 && (
        <p className="text-sm font-medium text-muted-foreground">
          (まだデプロイがありません。git push / `tbm deploy --local` / `tbm deploy --image` で開始)
        </p>
      )}

      {deploys && deploys.length > 0 && (
        <ul className="flex flex-col gap-2">
          {deploys.map((d) => (
            <li
              key={d.id}
              className="flex flex-wrap items-center justify-between gap-3 rounded-2xl border-2 border-[#e8e2d6] bg-card px-4 py-3"
            >
              <div className="flex min-w-0 flex-col gap-0.5">
                <span className="truncate font-bold text-foreground">
                  <StatusDot status={d.status} />
                  {/* 「稼働中」は行の状態なので左のステータス側に置く。右をボタン専用にしないと
                      バッジの幅の分だけボタンが行ごとにずれて見える。 */}
                  {d.id === servingDeployId && (
                    <span className="mr-1.5 rounded-full bg-accent px-2 py-0.5 text-xs font-bold text-accent-foreground">
                      稼働中
                    </span>
                  )}
                  {d.commit_message || d.git_sha}
                </span>
                <span className="truncate text-xs font-medium text-muted-foreground">
                  {new Date(d.created_at).toLocaleString("ja-JP")} · {d.git_sha} ·{" "}
                  {shortDigest(d.image_digest)}
                </span>
                {d.error && <span className="text-xs font-semibold text-[#e05a5a]">{d.error}</span>}
              </div>
              {d.status === "succeeded" && (
                <Button
                  type="default"
                  size="small"
                  loading={rollback.isPending}
                  onClick={() => rollback.mutate(d.id)}
                >
                  {d.id === servingDeployId ? "このイメージで再デプロイ" : "このデプロイに戻す"}
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// status の小さな色ドット + ラベル。
function StatusDot({ status }: { status: Deploy["status"] }) {
  const color =
    status === "succeeded" ? "bg-[#3f8a55]" : status === "failed" ? "bg-[#e05a5a]" : "bg-[#b5862a]"; // received / pulling / starting
  return (
    <span className="mr-1 inline-flex items-center gap-1.5">
      <span className={`size-2 rounded-full ${color}`} />
      <span className="text-xs font-semibold text-muted-foreground">
        {deployStatusLabel(status)}
      </span>
    </span>
  );
}
