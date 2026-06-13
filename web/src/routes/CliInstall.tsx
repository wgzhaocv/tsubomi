import { Link } from "react-router";

import { Code, InstallSteps } from "@/components/install-steps";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";

// tbm CLI のインストール手順ページ(単体)。管理画面の「はじめに」と同じ手順を
// 共有コンポーネント(InstallSteps)で表示する。ログイン不要で開ける(PKCE フローや
// 直リンクからの導線用)。
export default function CliInstall() {
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

      <div className="w-full max-w-2xl">
        <InstallSteps />
      </div>

      <Button asChild type="text" size="small">
        <Link to="/">← 戻る</Link>
      </Button>
    </main>
  );
}
