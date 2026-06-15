import { type ReactNode, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { CodeBlock } from "@/components/ui/codeblock";
import { useCopied } from "@/lib/use-copied";
import { cn } from "@/lib/utils";

// tbm CLI のインストール手順(各 OS のコマンド)。はじめに(Welcome)と単体の /cli
// ページの両方から使うため、**コマンドの正本をここに 1 つだけ置く**(installTargets)。
// コマンド中のドメインは window.location.origin から組むので、どのデプロイ先でも正しい。

// 文章中のインライン・コード(tbm login 等)の共通スタイル。
export function Code({ children }: { children: ReactNode }) {
  return (
    <code className="rounded-md bg-card/80 px-1.5 py-0.5 text-[0.85em] font-bold text-foreground">
      {children}
    </code>
  );
}

export type OsKey = "unix" | "ps" | "cmd";

export interface InstallTarget {
  key: OsKey;
  /** OS 切り替えタブのラベル(macOS / Linux など)。 */
  label: string;
  /** コマンドを貼り付ける「窓」の名前(非エンジニア向け。bash 等の専門語は使わない)。 */
  terminal: string;
  note: string;
  command: string;
}

// インストールコマンドの正本。origin から組むのでドメイン非依存。
export function installTargets(): InstallTarget[] {
  const origin = window.location.origin;
  return [
    {
      key: "unix",
      label: "macOS / Linux",
      terminal: "ターミナル",
      note: "~/.tbm/bin に入れて、PATH を通します(管理者権限は不要)。末尾の exec $SHELL でそのまま tbm が使えます。",
      command: `curl -fsSL ${origin}/install.sh | sh && exec $SHELL`,
    },
    {
      key: "ps",
      label: "Windows(PowerShell)",
      terminal: "PowerShell",
      note: "%LOCALAPPDATA%\\tbm\\bin に入れて、ユーザ PATH に追加します(管理者権限は不要)。",
      command: `irm ${origin}/install.ps1 | iex`,
    },
    {
      key: "cmd",
      label: "Windows(cmd)",
      terminal: "コマンドプロンプト",
      note: "PowerShell を使わない方向け。同じ場所に入ります。",
      command: `curl -fsSL ${origin}/install.bat -o %TEMP%\\tbm-install.bat && %TEMP%\\tbm-install.bat`,
    },
  ];
}

// 実行環境から既定の OS を推測する(はじめにの導線で「まず 1 つだけ」見せるため)。
export function detectOs(): OsKey {
  if (typeof navigator !== "undefined" && /Win/i.test(navigator.userAgent)) return "ps";
  return "unix";
}

// はじめに用:自分の OS のコマンドを 1 つだけ見せ、他は小さなタブで切り替える。
// CodeBlock(温かみのある端末風 + コピー)で 1 枚だけ表示してすっきり見せる。
export function InstallPicker({ className }: { className?: string }) {
  const targets = installTargets();
  const [active, setActive] = useState<OsKey>(detectOs());
  const cur = targets.find((t) => t.key === active) ?? targets[0];
  return (
    <div className={cn("flex w-full flex-col gap-2.5", className)}>
      <div className="flex flex-wrap gap-1.5">
        {targets.map((t) => {
          const on = t.key === active;
          return (
            <button
              key={t.key}
              type="button"
              onClick={() => setActive(t.key)}
              aria-pressed={on}
              className={cn(
                "rounded-full px-3 py-1 text-xs font-bold outline-none transition-colors focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-2",
                on ? "bg-[#0CC0B5] text-[#FFF9E3]" : "bg-card text-foreground/65 hover:bg-card/70",
              )}
            >
              {t.label}
            </button>
          );
        })}
      </div>
      <CodeBlock code={cur.command} title={cur.terminal} />
      <p className="text-xs leading-relaxed font-medium text-foreground/65">{cur.note}</p>
    </div>
  );
}

function CommandCard({ title, note, command }: { title: string; note: string; command: string }) {
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

// 単体 /cli ページ用:全 OS のコマンドカードを並べる(網羅表示)。
export function InstallSteps({ className }: { className?: string }) {
  return (
    <div className={cn("flex w-full flex-col items-stretch gap-4", className)}>
      {installTargets().map((t) => (
        <CommandCard key={t.key} title={t.label} note={t.note} command={t.command} />
      ))}
      <p className="text-center text-xs font-medium text-foreground/70">
        対応プラットフォーム:macOS(Apple Silicon)/ Linux(x86_64・arm64)/ Windows(x86_64)。更新は{" "}
        <Code>tbm update</Code>
        (新しいバージョンがあればコマンド実行後に通知が出ます)。
      </p>
    </div>
  );
}
