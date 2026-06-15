import { type ReactNode } from "react";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { useCopied } from "@/lib/use-copied";
import { cn } from "@/lib/utils";

// tbm CLI のインストール手順(各 OS のコマンドカード)。Welcome(管理画面の入口)と
// 単体の /cli ページの両方から使うため、コマンドの正本をここに 1 つだけ置く。
// コマンド中のドメインは window.location.origin から組むので、どのデプロイ先でも正しい。

// 文章中のインライン・コード(tbm login 等)の共通スタイル。
export function Code({ children }: { children: ReactNode }) {
  return (
    <code className="rounded-md bg-card/80 px-1.5 py-0.5 text-[0.85em] font-bold text-foreground">
      {children}
    </code>
  );
}

function CommandCard({
  title,
  note,
  command,
}: {
  title: string;
  note: string;
  command: string;
}) {
  const { copied, copy } = useCopied();
  return (
    <Card className="w-full">
      <CardHeader>
        <CardTitle className="text-base">{title}</CardTitle>
        <CardAction>
          <Button type="default" size="small" onClick={() => copy(command)}>
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

export function InstallSteps({ className }: { className?: string }) {
  const origin = window.location.origin;
  return (
    <div className={cn("flex w-full flex-col items-stretch gap-4", className)}>
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
        note="PowerShell を使わない cmd 版。同じ場所に入ります。"
        command={`curl -fsSL ${origin}/install.bat -o %TEMP%\\tbm-install.bat && %TEMP%\\tbm-install.bat`}
      />

      <p className="text-center text-xs font-medium text-foreground/70">
        対応プラットフォーム:macOS(Apple Silicon)/ Linux(x86_64・arm64)/
        Windows(x86_64)。更新は <Code>tbm update</Code>
        (新しいバージョンがあればコマンド実行後に通知が出ます)。
      </p>
    </div>
  );
}
