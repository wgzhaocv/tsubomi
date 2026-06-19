import { type ReactNode } from "react";
import { Link } from "react-router";

import { ClaudeSession, type SessionLine } from "@/components/claude-session";
import { Code, InstallPicker } from "@/components/install-steps";
import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/codeblock";
import { Divider } from "@/components/ui/divider";
import { Title } from "@/components/ui/title";
import { Typewriter } from "@/components/ui/typewriter";
import { useMeQuery } from "@/lib/auth";
import { RESOURCES } from "@/lib/resources";

// はじめに(管理画面の入口)。コマンド 1 本から公開 URL まで、利用者を **1 本の線**で
// 上から下へ導く。Claude Code とのやり取りは端末風パネルで打字機デモ(視口に入ると再生)。
// 会話・アプリ名・URL はすべて **例**(末尾で明示)で、実物には見せかけない。全体は簡単に。

// Claude Code に話しかけてからデプロイ、公開 URL が返るまでの一連のデモ(すべて例)。
const DEMO: SessionLine[] = [
  { role: "user", text: "社内の出欠管理アプリを作って" },
  { role: "claude", text: "作って、動作確認まで進めますね…" },
  { role: "claude", text: "✓ アプリができました。" },
  { role: "user", text: "作ったアプリを tbm でデプロイして" },
  { role: "claude", text: "tbm でデプロイしています…" },
  { role: "claude", text: "✓ 公開しました。URL はこちらです:" },
  { role: "url" },
];

// タイムラインの 1 ステップ(左に番号ノード + 連結線、右に内容)。
function Step({
  n,
  title,
  last,
  children,
}: {
  n: number;
  title: string;
  last?: boolean;
  children: ReactNode;
}) {
  return (
    <li className="relative flex gap-4 pb-9 last:pb-0">
      {!last && (
        <span
          aria-hidden
          className="absolute top-10 -bottom-1 left-[17px] w-0.5 rounded-full bg-[#dcd1b6]"
        />
      )}
      <span className="z-10 grid size-9 shrink-0 place-items-center rounded-full bg-[#0CC0B5] text-sm font-black text-[#FFF9E3] shadow-[0_3px_0_0_#0a9e95]">
        {n}
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-2.5 pt-1">
        <h2 className="text-base font-bold text-foreground">{title}</h2>
        {children}
      </div>
    </li>
  );
}

// 最後のステップ:ブラウザで開いたイメージ(図解。本物のスクショではない)。
function BrowserMock() {
  return (
    <div className="overflow-hidden rounded-2xl border-2 border-[#c4b89e] shadow-[0_4px_0_0_#d8ccae]">
      <div className="flex items-center gap-2 border-b-2 border-[#e8e2d6] bg-card px-3 py-2">
        <span className="size-2.5 rounded-full bg-[#e87878]" />
        <span className="size-2.5 rounded-full bg-[#f5c31c]" />
        <span className="size-2.5 rounded-full bg-[#7cc47c]" />
        <span className="ml-1.5 flex-1 truncate rounded-md bg-secondary px-2.5 py-1 font-mono text-xs font-medium text-foreground/70">
          my-app.{window.location.host}
        </span>
      </div>
      <div className="flex flex-col items-center gap-1 bg-[#fffdf5] px-4 py-7 text-center">
        <span className="text-3xl">🌷</span>
        <p className="text-sm font-bold text-foreground">あなたのアプリが公開されました</p>
        <p className="text-xs font-medium text-foreground/60">この URL を開くだけ。</p>
      </div>
    </div>
  );
}

export default function Welcome() {
  const { data: me } = useMeQuery();
  const greetingName = me?.name ?? me?.email ?? "ようこそ";

  return (
    <PageContainer>
      <div className="mx-auto flex max-w-2xl flex-col gap-7">
        <PageMeta title="はじめに" />

        <header className="flex flex-col gap-2.5">
          <Title size="large" color="app-teal" className="self-start">
            はじめに
          </Title>
          <h1 className="text-2xl font-extrabold tracking-tight text-foreground">
            ようこそ、{greetingName} さん 🌷
          </h1>
          <p className="text-sm font-semibold text-foreground/75">
            <Typewriter>コマンド 1 本から、公開 URL まで。</Typewriter>
          </p>
        </header>

        <Divider type="dashed-teal" />

        <ol className="flex flex-col">
          <Step n={1} title="tbm CLI をインストール">
            <p className="text-sm font-medium text-foreground/75">
              お使いの OS のコマンドをコピーして、下の窓(ターミナル)に貼り付けて実行します。
            </p>
            <InstallPicker />
          </Step>

          <Step n={2} title="ログインする">
            <p className="text-sm font-medium text-foreground/75">
              同じ窓で次を実行し、ブラウザで「許可する」を押すだけ(SSH 先などは自動でコピペ方式に
              切り替わります)。
            </p>
            <CodeBlock code="tbm login" showCopy />
          </Step>

          <Step n={3} title="Claude Code を起動">
            <p className="text-sm font-medium text-foreground/75">
              自分のプロジェクトのフォルダ、あるいはどこでも好きなフォルダでターミナルを開いて、
              Claude Code を起動します。
            </p>
            <CodeBlock code="claude" showCopy />
          </Step>

          <Step n={4} title="あとは Claude Code に頼むだけ">
            <p className="text-sm font-medium text-foreground/75">
              起動した Claude Code に、作りたいものを話しかけます。できたら「
              <strong className="font-bold text-foreground">tbm でデプロイして</strong>
              」と頼むと、公開 URL が返ってきます。
            </p>
            <ClaudeSession script={DEMO} />
          </Step>

          <Step n={5} title="リンクを開く" last>
            <p className="text-sm font-medium text-foreground/75">
              返ってきた URL をブラウザで開けば、もう世界に公開されています。
            </p>
            <BrowserMock />
          </Step>
        </ol>

        <p className="text-xs leading-relaxed font-medium text-muted-foreground">
          ※ ステップ 4・5 の会話・アプリ名・URL はイメージです。実際はあなたが Claude Code に
          頼む内容によって変わります。ステップ 1〜3 のコマンドはそのまま使えます。
        </p>

        <Divider type="line-brown" />

        <section className="flex flex-col gap-3">
          <h2 className="text-base font-bold text-foreground">リソースを見る</h2>
          <div className="flex flex-wrap gap-2">
            {RESOURCES.map((r) => {
              const Icon = r.icon;
              return (
                <Button
                  key={r.path}
                  asChild
                  type="default"
                  size="small"
                  icon={<Icon className="size-4" />}
                >
                  <Link to={r.path}>{r.label}</Link>
                </Button>
              );
            })}
          </div>
          <p className="text-xs font-medium text-muted-foreground">
            web からも <Code>tbm</Code> CLI からも、同じリソースを操作できます。
          </p>
        </section>
      </div>
    </PageContainer>
  );
}
