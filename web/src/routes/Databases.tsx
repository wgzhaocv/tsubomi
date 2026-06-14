import { useState } from "react";
import { Database, Plus } from "lucide-react";
import { useNavigate } from "react-router";

import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Divider } from "@/components/ui/divider";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { Title } from "@/components/ui/title";
import { useCreateDatabase, useDatabases } from "@/lib/databases";

// データベース一覧。RESOURCES(サイドメニュー)の「データベース」項目に対応する
// 実画面。作成は名前を 1 つ入れるだけ(平台が wire 名・role・パスワードを生成する)。

export default function Databases() {
  const navigate = useNavigate();
  const { data: dbs, isPending, error } = useDatabases();
  const create = useCreateDatabase();

  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");

  const submit = () => {
    const trimmed = name.trim();
    if (!trimmed) return;
    create.mutate(trimmed, {
      onSuccess: (db) => {
        setOpen(false);
        setName("");
        navigate(`/databases/${db.id}`);
      },
    });
  };

  return (
    <PageContainer>
      <div className="flex flex-col gap-7">
        <PageMeta title="データベース" />

        <header className="flex flex-wrap items-center justify-between gap-4">
          <Title size="large" color="app-blue">
            データベース
          </Title>
          {/* 空のときは下の空状態 CTA に任せ、1 つ以上あるときだけ右上に出す。 */}
          {dbs && dbs.length > 0 && (
            <Button type="default" icon={<Plus className="size-4" />} onClick={() => setOpen(true)}>
              データベースを作成
            </Button>
          )}
        </header>

        <Divider type="line-brown" />

        {error && (
          <p className="text-sm font-semibold text-[#e05a5a]">
            読み込みに失敗しました:{error.message}
          </p>
        )}

        {!isPending && dbs && dbs.length === 0 && (
          <Card type="dashed">
            <CardContent className="flex flex-col items-center gap-4 px-6 py-12 text-center">
              <div className="grid size-16 place-items-center rounded-full bg-accent text-accent-foreground">
                <Database className="size-8" />
              </div>
              <div className="flex flex-col gap-1.5">
                <p className="text-lg font-bold text-foreground">まだデータベースがありません</p>
                <p className="max-w-md text-sm font-medium text-muted-foreground">
                  単一インスタンス上に独立した PostgreSQL
                  データベースを作成します。接続文字列はここから確認・コピーできます。
                </p>
              </div>
              <Button
                type="primary"
                icon={<Plus className="size-4" />}
                onClick={() => setOpen(true)}
              >
                データベースを作成
              </Button>
            </CardContent>
          </Card>
        )}

        {dbs && dbs.length > 0 && (
          <ul className="flex flex-col gap-3">
            {dbs.map((db) => (
              <li key={db.id}>
                <Card
                  interactive
                  onClick={() => navigate(`/databases/${db.id}`)}
                  className="flex-row items-center justify-between gap-4 py-4"
                >
                  <CardContent className="flex min-w-0 items-center gap-3.5">
                    <div className="grid size-11 shrink-0 place-items-center rounded-2xl bg-accent text-accent-foreground">
                      <Database className="size-5.5" />
                    </div>
                    <div className="flex min-w-0 flex-col">
                      <span className="truncate text-base font-bold text-foreground">
                        {db.display_name}
                      </span>
                      <span className="truncate text-xs font-medium text-muted-foreground">
                        database{db.anon_seq} · 作成{" "}
                        {new Date(db.created_at).toLocaleDateString("ja-JP")}
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
          title="データベースを作成"
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
          <div className="flex w-full flex-col gap-3">
            <Input
              label="名前"
              placeholder="例:myapp-db"
              value={name}
              autoFocus
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") submit();
              }}
              description="表示名です。後から変えても接続文字列は変わりません。"
            />
            {create.error && (
              <p className="text-sm font-semibold text-[#e05a5a]">{create.error.message}</p>
            )}
          </div>
        </Modal>
      </div>
    </PageContainer>
  );
}
