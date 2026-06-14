import { RefreshCw } from "lucide-react";
import { useParams } from "react-router";

import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/codeblock";
import { useService, useServiceLogs } from "@/lib/services";

// ログ:走っているコンテナの tail を表示する。tab 表示中は数秒ごとに自動更新
// (useServiceLogs の refetchInterval)+ 手動更新ボタン。コンテナが無い(stopped /
// 未デプロイ)ときはサーバが空を返すので、その旨を表示する。
export default function ServiceLogs() {
  const { id = "" } = useParams();
  const { data: svc } = useService(id);
  // 走っている時だけ polling(止まっているサービスを 5 秒ごとに叩かない)。
  const { data, isPending, isFetching, error, refetch } = useServiceLogs(
    id,
    svc?.phase === "running",
  );
  const logs = data?.logs ?? "";

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-lg font-bold text-foreground">ログ</h2>
        <Button
          type="default"
          size="small"
          icon={<RefreshCw className="size-4" />}
          loading={isFetching}
          onClick={() => refetch()}
        >
          更新
        </Button>
      </div>
      <p className="text-sm font-medium text-muted-foreground">
        コンテナ標準出力の末尾です(数秒ごとに自動更新)。
      </p>

      {error && <p className="text-sm font-semibold text-[#e05a5a]">{error.message}</p>}

      {!isPending && logs.trim() === "" ? (
        <p className="text-sm font-medium text-muted-foreground">
          (ログがありません。コンテナが走っていない可能性があります)
        </p>
      ) : (
        <CodeBlock title="stdout" language="log" code={logs || "…"} showCopy />
      )}
    </div>
  );
}
