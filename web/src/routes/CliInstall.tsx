import { useState } from "react";
import { Link } from "react-router";

// tbm CLI のインストール手順ページ(プレースホルダ品質)。
// コマンド中のドメインは window.location.origin から組むので、
// どのデプロイ先でもこのページはそのまま正しい。
function CommandCard({ title, note, command }: { title: string; note: string; command: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="w-full max-w-2xl rounded-lg border p-4">
      <div className="mb-1 flex items-center justify-between">
        <h2 className="font-medium">{title}</h2>
        <button
          onClick={() => {
            void navigator.clipboard.writeText(command).then(() => {
              setCopied(true);
              setTimeout(() => setCopied(false), 1500);
            });
          }}
          className="rounded-md border px-2 py-0.5 text-xs text-muted-foreground hover:bg-muted"
        >
          {copied ? "コピーしました ✓" : "コピー"}
        </button>
      </div>
      <p className="mb-2 text-xs text-muted-foreground">{note}</p>
      <code className="block overflow-x-auto rounded-md bg-muted px-3 py-2 text-xs whitespace-pre">
        {command}
      </code>
    </div>
  );
}

export default function CliInstall() {
  const origin = window.location.origin;
  return (
    <main className="flex min-h-dvh flex-col items-center gap-5 bg-background p-8 text-foreground">
      <h1 className="text-2xl font-semibold">tbm CLI のインストール</h1>
      <p className="text-sm text-muted-foreground">
        インストール後は <code>tbm login</code> で認証します。アンインストールは{" "}
        <code>tbm uninstall</code>(設定・PATH・本体まで残留物ゼロで消えます)。
      </p>

      <CommandCard
        title="macOS / Linux"
        note="~/.tbm/bin に入れて、シェルの rc に PATH を追記します(sudo 不要)。末尾の exec $SHELL がシェルを再起動するので、そのまま tbm が使えます。"
        command={`curl -fsSL ${origin}/install.sh | sh && exec $SHELL`}
      />
      <CommandCard
        title="Windows — PowerShell"
        note="%LOCALAPPDATA%\\tbm\\bin に入れて、ユーザ PATH に追加します(管理者権限不要)。"
        command={`irm ${origin}/install.ps1 | iex`}
      />
      <CommandCard
        title="Windows — コマンドプロンプト(cmd)"
        note="PowerShell を使わない純 cmd 版。同じ場所に入ります。"
        command={`curl -fsSL ${origin}/install.bat -o %TEMP%\\tbm-install.bat && %TEMP%\\tbm-install.bat`}
      />

      <p className="text-xs text-muted-foreground">
        対応プラットフォーム:macOS(Apple Silicon)/ Linux(x86_64・arm64)/ Windows(x86_64)。
        更新は <code>tbm update</code>(新版があればコマンド実行後に通知が出ます)。
      </p>

      <Link to="/" className="text-sm text-muted-foreground underline-offset-4 hover:underline">
        ← 戻る
      </Link>
    </main>
  );
}
