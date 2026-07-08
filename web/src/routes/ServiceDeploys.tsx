import { useParams } from "react-router";

import { Button } from "@/components/ui/button";
import {
  type Deploy,
  deployStatusLabel,
  shortDigest,
  useRollbackService,
  useServiceDeploys,
} from "@/lib/services";

// デプロイ履歴。各 succeeded 行は「このデプロイに戻す」(rollback = 旧 digest を再起動、再 build なし)。
export default function ServiceDeploys() {
  const { id = "" } = useParams();
  const { data: deploys, isPending, error } = useServiceDeploys(id);
  const rollback = useRollbackService(id);

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
                <Button
                  type="default"
                  size="small"
                  loading={rollback.isPending}
                  onClick={() => rollback.mutate(d.id)}
                >
                  このデプロイに戻す
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
