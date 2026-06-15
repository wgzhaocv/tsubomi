import { Outlet } from "react-router";

import { PageContainer } from "@/components/page-container";
import { Divider } from "@/components/ui/divider";
import { Title } from "@/components/ui/title";
import { useMeQuery } from "@/lib/auth";

// owner 専用ルートの守衛。owner 以外には「owner だけ」の代替画面を出す。
// 表示制御はただの UX — 後端が 403 で守る(設計 v2 §7)。サイドメニューも
// owner 限定で出すので、ここに来るのは URL 直打ち / 降格時くらい。
// me 取得中(undefined)は子を描画(各ページが自分で読み込み表示する)。
export function RequireOwner() {
  const { data: me } = useMeQuery();

  if (me && me.role !== "owner") {
    return (
      <PageContainer>
        <div className="flex flex-col gap-7">
          <Title size="large" color="purple">
            管理画面
          </Title>
          <Divider type="line-brown" />
          <p className="text-sm font-semibold text-[#e05a5a]">
            この画面は管理者だけが利用できます。
          </p>
        </div>
      </PageContainer>
    );
  }

  return <Outlet />;
}
