import { type ReactNode, useState } from "react";
import { Link } from "react-router";

import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";

// tbm CLI のインストール手順ページ(プレースホルダ品質)。
// コマンド中のドメインは window.location.origin から組むので、
// どのデプロイ先でもこのページはそのまま正しい。

// 文章中のインライン・コード(tbm login 等)の共通スタイル。
function Code({ children }: { children: ReactNode }) {
  return (
    <code className="rounded-md bg-card/80 px-1.5 py-0.5 text-[0.85em] font-bold text-foreground">
      {children}
    </code>
  );
}

function CommandCard({ title, note, command }: { title: string; note: string; command: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Card className="w-full max-w-2xl">
      <CardHeader>
        <CardTitle className="text-base">{title}</CardTitle>
        <CardAction>
          <Button
            type="default"
            size="small"
            onClick={() => {
              void navigator.clipboard.writeText(command).then(() => {
                setCopied(true);
                setTimeout(() => setCopied(false), 1500);
              });
            }}
          >
            {copied ? "コピーしました ✓" : "コピー"}
          </Button>
        </CardAction>
        <CardDescription>{note}</CardDescription>
      </CardHeader>
      <CardContent>
        <code className="block overflow-x-auto rounded-xl bg-secondary px-3 py-2.5 text-xs whitespace-pre text-foreground/90">
          {command}
        </code>
      </CardContent>
    </Card>
  );
}

export default function CliInstall() {
  const origin = window.location.origin;
  return (
    <main className="flex min-h-dvh flex-col items-center gap-5 p-8 text-foreground">
      <PageMeta title="tbm CLI のインストール" description="tbm CLI のインストール手順" />
      <div className="flex max-w-2xl flex-col items-center gap-2 text-center">
        <h1 className="text-3xl font-extrabold tracking-tight">tbm CLI のインストール</h1>
        <p className="text-sm font-medium text-foreground/75">
          インストール後は <Code>tbm login</Code> で認証します。アンインストールは{" "}
          <Code>tbm uninstall</Code>(設定・PATH・本体まで残留物ゼロで消えます)。
        </p>
      </div>

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

      <p className="max-w-2xl text-center text-xs font-medium text-foreground/70">
        対応プラットフォーム:macOS(Apple Silicon)/ Linux(x86_64・arm64)/ Windows(x86_64)。更新は{" "}
        <Code>tbm update</Code>
        (新版があればコマンド実行後に通知が出ます)。
      </p>

      <Button asChild type="text" size="small">
        <Link to="/">← 戻る</Link>
      </Button>
    </main>
  );
}
