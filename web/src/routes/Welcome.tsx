import { Link } from "react-router";

import { Code, InstallSteps } from "@/components/install-steps";
import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Divider } from "@/components/ui/divider";
import { Title } from "@/components/ui/title";
import { useMeQuery } from "@/lib/auth";
import { RESOURCES } from "@/lib/resources";

// 手順番号のバッジ(ミント丸 + クリーム数字)。
function StepBadge({ n }: { n: number }) {
  return (
    <span className="mr-2 inline-grid size-6 place-items-center rounded-full bg-[#0CC0B5] text-xs font-black text-[#FFF9E3]">
      {n}
    </span>
  );
}

// 管理画面の入口(はじめに)。ログイン直後に最初に見える画面。
// web / CLI どちらからでも操作できるが、CLI を使う人向けに導入手順も置く。
// 利用者名はサーバ状態 → useMeQuery を直接読む(props で受け取らない)。
export default function Welcome() {
  const { data: me } = useMeQuery();
  const greetingName = me?.name ?? me?.email ?? "ようこそ";

  return (
    // はじめに は「文章」ページなので、5xl の内容領域の中で 3xl の列を中央寄せにする
    // (左右の余白を対称にし、左に寄って右が大きく空くのを防ぐ)。
    <PageContainer>
      <div className="mx-auto flex max-w-3xl flex-col gap-8">
        <PageMeta title="はじめに" />

        <header className="flex flex-col gap-3">
          <Title size="large" color="app-teal" className="self-start">
            はじめに
          </Title>
          <h1 className="text-2xl font-extrabold tracking-tight text-foreground">
            ようこそ、{greetingName} さん 🌷
          </h1>
          <p className="text-sm leading-relaxed font-medium text-foreground/75">
            つぼみは web と <Code>tbm</Code> CLI のどちらからでも操作できます。CLI を使うなら、
            下の手順で自分の PC にインストールし、<Code>tbm login</Code> で認証してください。
            左のメニューから サービス・データベース・ボリューム・キャッシュを確認できます。
          </p>
        </header>

        <Divider type="dashed-teal" />

        {/* 1. インストール */}
        <section className="flex flex-col gap-4">
          <h2 className="text-base font-bold text-foreground">
            <StepBadge n={1} />
            tbm CLI をインストール
          </h2>
          <InstallSteps />
        </section>

        {/* 2. 認証 */}
        <section className="flex flex-col gap-2">
          <h2 className="text-base font-bold text-foreground">
            <StepBadge n={2} />
            ログインする
          </h2>
          <p className="text-sm font-medium text-foreground/75">
            ターミナルで <Code>tbm login</Code> を実行します。ブラウザで「許可する」を
            押すだけで認証が完了します(SSH 先・ヘッドレスでは自動でコピペ方式に 切り替わります)。
          </p>
        </section>

        <Divider type="line-brown" />

        {/* 次のステップ:リソースへの導線 */}
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
            まだ何も作成していなければ、各メニューは空の状態で表示されます。
          </p>
        </section>
      </div>
    </PageContainer>
  );
}
