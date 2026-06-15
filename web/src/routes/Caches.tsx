import { useState } from "react";
import { Plus, Zap } from "lucide-react";
import { useNavigate } from "react-router";

import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Divider } from "@/components/ui/divider";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { Title } from "@/components/ui/title";
import { useCaches, useCreateCache } from "@/lib/caches";

// キャッシュ一覧。RESOURCES(サイドメニュー)の「キャッシュ」項目に対応する実画面。
// 作成は名前を 1 つ入れるだけ(平台が ACL ユーザ名・namespace・パスワードを生成する)。
// 詳細ページ(接続文字列の表示 / rotate / 削除)は S3 で足す — S1 は一覧 + 作成まで。

export default function Caches() {
  const navigate = useNavigate();
  const { data: caches, isPending, error } = useCaches();
  const create = useCreateCache();

  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");

  const submit = () => {
    const trimmed = name.trim();
    if (!trimmed || create.isPending) return; // 二重送信を防ぐ(連打 / Enter+クリック)
    create.mutate(trimmed, {
      onSuccess: (cache) => {
        setOpen(false);
        setName("");
        navigate(`/caches/${cache.id}`);
      },
    });
  };

  return (
    <PageContainer>
      <div className="flex flex-col gap-7">
        <PageMeta title="キャッシュ" />

        <header className="flex flex-wrap items-center justify-between gap-4">
          <Title size="large" color="app-orange">
            キャッシュ
          </Title>
          {/* 空のときは下の空状態 CTA に任せ、1 つ以上あるときだけ右上に出す。 */}
          {caches && caches.length > 0 && (
            <Button type="default" icon={<Plus className="size-4" />} onClick={() => setOpen(true)}>
              キャッシュを作成
            </Button>
          )}
        </header>

        <Divider type="line-brown" />

        {error && (
          <p className="text-sm font-semibold text-[#e05a5a]">
            読み込みに失敗しました:{error.message}
          </p>
        )}

        {!isPending && caches && caches.length === 0 && (
          <Card type="dashed">
            <CardContent className="flex flex-col items-center gap-4 px-6 py-12 text-center">
              <div className="grid size-16 place-items-center rounded-full bg-accent text-accent-foreground">
                <Zap className="size-8" />
              </div>
              <div className="flex flex-col gap-1.5">
                <p className="text-lg font-bold text-foreground">まだキャッシュがありません</p>
                <p className="max-w-md text-sm font-medium text-muted-foreground">
                  サービスに注入して使う Valkey の高速キャッシュです。接続文字列を env
                  に注入して利用します。
                </p>
              </div>
              <Button
                type="primary"
                icon={<Plus className="size-4" />}
                onClick={() => setOpen(true)}
              >
                キャッシュを作成
              </Button>
            </CardContent>
          </Card>
        )}

        {caches && caches.length > 0 && (
          <ul className="flex flex-col gap-3">
            {caches.map((cache) => (
              <li key={cache.id}>
                <Card
                  interactive
                  onClick={() => navigate(`/caches/${cache.id}`)}
                  className="flex-row items-center justify-between gap-4 py-4"
                >
                  <CardContent className="flex min-w-0 items-center gap-3.5">
                    <div className="grid size-11 shrink-0 place-items-center rounded-2xl bg-accent text-accent-foreground">
                      <Zap className="size-5.5" />
                    </div>
                    <div className="flex min-w-0 flex-col">
                      <span className="truncate text-base font-bold text-foreground">
                        {cache.display_name}
                      </span>
                      <span className="truncate text-xs font-medium text-muted-foreground">
                        cache{cache.anon_seq} · 作成{" "}
                        {new Date(cache.created_at).toLocaleDateString("ja-JP")}
                      </span>
                    </div>
                  </CardContent>
                </Card>
              </li>
            ))}
          </ul>
        )}

        <Modal
          open={open}
          title="キャッシュを作成"
          typewriter={false}
          onClose={() => setOpen(false)}
          width={460}
          footer={
            <>
              <Button type="text" onClick={() => setOpen(false)}>
                キャンセル
              </Button>
              <Button type="primary" loading={create.isPending} onClick={submit}>
                作成
              </Button>
            </>
          }
        >
          {/* 本物の form。Enter は onSubmit を 1 回だけ通す。 */}
          <form
            onSubmit={(e) => {
              e.preventDefault();
              submit();
            }}
            className="flex w-full flex-col gap-3"
          >
            <Input
              label="名前"
              placeholder="例:myapp-cache"
              value={name}
              autoFocus
              onChange={(e) => setName(e.target.value)}
              description="表示名です。後から変えても接続文字列は変わりません。"
            />
            {create.error && (
              <p className="text-sm font-semibold text-[#e05a5a]">{create.error.message}</p>
            )}
          </form>
        </Modal>
      </div>
    </PageContainer>
  );
}
