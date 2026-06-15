import { useState } from "react";
import { Outlet } from "react-router";

import { PageContainer } from "@/components/page-container";
import { Button } from "@/components/ui/button";
import { Divider } from "@/components/ui/divider";
import { Input } from "@/components/ui/input";
import { Title } from "@/components/ui/title";
import { useViewerLogin } from "@/lib/admin";
import { useMeQuery } from "@/lib/auth";

// 閲覧ルート(管制面 / 使用量ランキング)の守衛。設計 v2 §7「見るは共有密码」:
// owner はそのまま、それ以外のログインユーザは共有パスワードを入れると只读で見られる。
// owner || is_viewer なら子を描画、さもなくば**この場で**解錠フォームを出す。
// 表示制御はただの UX — 後端の require_viewer_web が本丸(危険操作は別途 owner のみ)。
export function RequireViewer() {
  const { data: me } = useMeQuery();
  const login = useViewerLogin();
  const [password, setPassword] = useState("");

  // me 取得中(undefined)は子に任せる(各ページが自分で読み込み表示する)。
  if (!me || me.role === "owner" || me.is_viewer) {
    return <Outlet />;
  }

  // 成功すると me が無効化され is_viewer が翻る → 再描画で Outlet に切り替わる。
  const submit = () => {
    const pw = password.trim();
    if (!pw || login.isPending) return;
    login.mutate(pw);
  };

  return (
    <PageContainer>
      <div className="flex max-w-md flex-col gap-7">
        <Title size="large" color="purple">
          管理画面(閲覧)
        </Title>
        <Divider type="line-brown" />
        <p className="text-sm font-medium text-foreground">
          共有パスワードを入力すると、管制面を <strong>閲覧専用</strong> で見られます
          (8 時間有効)。資源の停止 / 削除など操作は管理者のみです。
        </p>
        <form
          onSubmit={(ev) => {
            ev.preventDefault();
            submit();
          }}
          className="flex w-full flex-col gap-3"
        >
          <Input
            label="共有パスワード"
            type="password"
            placeholder="••••••••"
            value={password}
            autoFocus
            onChange={(ev) => setPassword(ev.target.value)}
          />
          {login.error && (
            <p className="text-sm font-semibold text-[#e05a5a]">{login.error.message}</p>
          )}
          <div>
            <Button type="primary" loading={login.isPending} onClick={submit}>
              閲覧する
            </Button>
          </div>
        </form>
      </div>
    </PageContainer>
  );
}
