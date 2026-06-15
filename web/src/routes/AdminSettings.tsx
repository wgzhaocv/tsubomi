import { useState } from "react";

import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Divider } from "@/components/ui/divider";
import { Input } from "@/components/ui/input";
import { Title } from "@/components/ui/title";
import { useSetViewerPassword, useViewerStatus } from "@/lib/admin";

// 共有パスワード設定(owner 専用)。設計 v2 §7「見るは共有密码」:owner がここで
// 共有パスワードを設定 / リセットする。リセットすると既存の閲覧 grant は全失効する
// (扩散しすぎたら作り直せる)。後端は require_owner_web で守る(表示制御は UX)。

function formatDate(iso: string | null): string {
  if (!iso) return "—";
  return new Date(iso).toLocaleString("ja-JP");
}

export default function AdminSettings() {
  const { data: status, isPending, error } = useViewerStatus();
  const setPw = useSetViewerPassword();
  const [password, setPassword] = useState("");

  const submit = () => {
    const pw = password.trim();
    if (!pw || setPw.isPending) return;
    setPw.mutate(pw, { onSuccess: () => setPassword("") });
  };

  const isSet = status?.set ?? false;

  return (
    <PageContainer>
      <div className="flex max-w-xl flex-col gap-7">
        <PageMeta title="共有パスワード設定" />

        <Title size="large" color="purple">
          共有パスワード設定
        </Title>

        <Divider type="line-brown" />

        <p className="text-sm font-medium text-foreground">
          共有パスワードを知っている社内ユーザは、管制面(総覧 / 使用量ランキング)を
          <strong> 閲覧専用</strong> で見られます(8 時間有効)。停止 / 削除など操作は
          管理者だけです。<strong>リセットすると既存の閲覧は全て無効</strong> になります。
        </p>

        {error && (
          <p className="text-sm font-semibold text-[#e05a5a]">
            状態の読み込みに失敗しました:{error.message}
          </p>
        )}

        {!error && (
          <Card>
            <CardContent className="flex flex-col gap-1 px-6 py-5">
              <span className="text-xs font-bold text-muted-foreground">現在の状態</span>
              {isPending ? (
                <span className="text-sm font-medium text-muted-foreground">読み込み中…</span>
              ) : isSet ? (
                <span className="text-sm font-semibold text-foreground">
                  設定済み(最終更新 {formatDate(status?.updated_at ?? null)}
                  {status?.updated_by_name ? ` · ${status.updated_by_name}` : ""})
                </span>
              ) : (
                <span className="text-sm font-semibold text-[#c08a2e]">
                  未設定(共有パスワードを設定するまで、管理者以外は管制面を見られません)
                </span>
              )}
            </CardContent>
          </Card>
        )}

        <form
          onSubmit={(ev) => {
            ev.preventDefault();
            submit();
          }}
          className="flex w-full flex-col gap-3"
        >
          <Input
            label={isSet ? "新しい共有パスワード" : "共有パスワード"}
            type="password"
            placeholder="••••••••"
            value={password}
            autoFocus
            onChange={(ev) => {
              setPassword(ev.target.value);
              // 入力し直したら前回の成功 / エラー表示を消す(mutation 状態をリセット)。
              if (!setPw.isIdle) setPw.reset();
            }}
            description="社内で共有する閲覧用パスワード(8 文字以上)。session / CLI token とは別物です。"
          />
          {setPw.error && (
            <p className="text-sm font-semibold text-[#e05a5a]">{setPw.error.message}</p>
          )}
          {setPw.isSuccess && (
            <p className="text-sm font-semibold text-[#0b9c93]">
              共有パスワードを更新しました(既存の閲覧は無効化されました)。
            </p>
          )}
          <div>
            <Button type="primary" loading={setPw.isPending} onClick={submit}>
              {isSet ? "リセットする" : "設定する"}
            </Button>
          </div>
        </form>
      </div>
    </PageContainer>
  );
}
