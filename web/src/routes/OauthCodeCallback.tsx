import { useState } from "react";
import { useSearchParams } from "react-router";

import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";

// PKCE ステップ 3:待機中の `tbm login` プロンプトに貼り付けるための
// ワンタイムコードを表示する。
export default function OauthCodeCallback() {
  const [params] = useSearchParams();
  const [copied, setCopied] = useState(false);
  const code = params.get("code") ?? "";

  return (
    <main className="flex min-h-dvh flex-col items-center justify-center gap-6 p-8 text-foreground">
      <PageMeta title="コードをコピー" />
      <h1 className="text-3xl font-extrabold tracking-tight">コードをコピー</h1>

      <Card className="w-full max-w-md">
        <CardContent className="flex flex-col items-center gap-4">
          <p className="text-center text-sm text-muted-foreground">
            ターミナルの{" "}
            <code className="rounded-md bg-card/80 px-1.5 py-0.5 text-[0.85em] font-bold text-foreground">
              paste code:
            </code>{" "}
            に貼り付けてください(10分間有効、1回限り)。
          </p>

          <code className="w-full rounded-xl bg-secondary px-4 py-3 text-center text-sm font-bold break-all text-foreground">
            {code || "(code がありません)"}
          </code>

          {code && (
            <Button
              block
              type={copied ? "default" : "primary"}
              onClick={() => {
                void navigator.clipboard.writeText(code).then(() => {
                  setCopied(true);
                });
              }}
            >
              {copied ? "コピーしました ✓" : "コピー"}
            </Button>
          )}
        </CardContent>
      </Card>
    </main>
  );
}
