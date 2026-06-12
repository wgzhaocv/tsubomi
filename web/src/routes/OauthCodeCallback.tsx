import { useState } from "react";
import { useSearchParams } from "react-router";

// PKCE ステップ 3:待機中の `tbm login` プロンプトに貼り付けるための
// ワンタイムコードを表示する。
export default function OauthCodeCallback() {
  const [params] = useSearchParams();
  const [copied, setCopied] = useState(false);
  const code = params.get("code") ?? "";

  return (
    <main className="flex min-h-dvh flex-col items-center justify-center gap-6 bg-background p-8 text-foreground">
      <h1 className="text-2xl font-semibold">コードをコピー</h1>
      <p className="text-sm text-muted-foreground">
        ターミナルの <code>paste code:</code> に貼り付けてください(10分間有効、1回限り)。
      </p>
      <code className="max-w-full break-all rounded-md border bg-muted px-4 py-3 text-sm">
        {code || "(code がありません)"}
      </code>
      {code && (
        <button
          onClick={() => {
            void navigator.clipboard.writeText(code).then(() => setCopied(true));
          }}
          className="rounded-md bg-primary px-6 py-2 text-primary-foreground transition hover:opacity-90"
        >
          {copied ? "コピーしました ✓" : "コピー"}
        </button>
      )}
    </main>
  );
}
