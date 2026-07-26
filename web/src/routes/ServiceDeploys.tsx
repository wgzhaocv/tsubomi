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
// **今動いている版はボタンの文言を変える** — 自分へ「戻す」は意味不明だが、**同じイメージの
// 再デプロイは必要な操作**(rotate 後・env / 注入の変更後は起動時解決をやり直す必要がある)なので
// 消さずに残す(codex 深審)。
export default function ServiceDeploys() {
  const { id = "" } = useParams();
  const { data: deploys, isPending, error } = useServiceDeploys(id);
  const { data: svc } = useService(id);
  const rollback = useRollbackService(id);
  // 今動いている digest。これと同じ版へ「戻す」のは無意味なのでボタンを出さない。
  // digest で見るのは、rollback が新しい deploy 行を作る = 同 digest の行が複数在り得るため
  // (最新行だけ隠すと、戻した先の行に無意味なボタンが残る)。**running の時だけ**判定する —
  // 停止中は image_digest が最後の版を保持したままなので、それを「稼働中」と呼ぶと嘘になるし、
  // その版を選び直して起こすのは有効な操作。未デプロイ時も undefined = 全行にボタン。
  const servingDigest = svc?.phase === "running" ? svc.image_digest : undefined;

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
                  <StatusDot status={d.status} /> {d.commit_message || d.git_sha}
                </span>
                <span className="truncate text-xs font-medium text-muted-foreground">
                  {new Date(d.created_at).toLocaleString("ja-JP")} · {d.git_sha} ·{" "}
                  {shortDigest(d.image_digest)}
                </span>
                {d.error && <span className="text-xs font-semibold text-[#e05a5a]">{d.error}</span>}
              </div>
              {d.status === "succeeded" && (
                <div className="flex items-center gap-2">
                  {d.image_digest === servingDigest && (
                    <span className="text-xs font-semibold text-muted-foreground">稼働中</span>
                  )}
                  <Button
                    type="default"
                    size="small"
                    loading={rollback.isPending}
                    onClick={() => rollback.mutate(d.id)}
                  >
                    {d.image_digest === servingDigest
                      ? "このイメージで再デプロイ"
                      : "このデプロイに戻す"}
                  </Button>
                </div>
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
