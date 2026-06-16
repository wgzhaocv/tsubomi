import { useState } from "react";
import { Plus, ShieldCheck, Trash2, UserPlus, Users } from "lucide-react";

import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Divider } from "@/components/ui/divider";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { Title } from "@/components/ui/title";
import { type AdminOwner, useAddOwner, useOwners, useRemoveOwner } from "@/lib/owners";

// owner 管理(owner 専用)。design v2 §7:最大 2 名の対等 owner、互いに外せるが自分は外せない
// (最低 1 名)、外された人へメール通知。env は冷启动种のみ — 運用中はここで増減する。
// 表示制御はただの UX(後端が require_owner_web で守る)。

const MAX_OWNERS = 2;

export default function AdminOwners() {
  const { data: owners, isPending, error } = useOwners();
  const add = useAddOwner();
  const remove = useRemoveOwner();

  const [open, setOpen] = useState(false);
  const [email, setEmail] = useState("");
  const [removeTarget, setRemoveTarget] = useState<AdminOwner | null>(null);

  // owner ゲートはルート単位の <RequireOwner>(router)に集約 — ここでは弾かない
  // (降格は親守衛が me の更新で再描画して受ける)。後端も require_owner_web で守る。
  const submit = () => {
    const trimmed = email.trim();
    if (!trimmed || add.isPending) return; // 二重送信を防ぐ
    add.mutate(trimmed, {
      onSuccess: () => {
        setOpen(false);
        setEmail("");
      },
    });
  };

  const canAdd = owners != null && owners.length < MAX_OWNERS;

  return (
    <PageContainer>
      <div className="flex flex-col gap-7">
        <PageMeta title="管理者管理" />

        <header className="flex flex-wrap items-center justify-between gap-4">
          <Title size="large" color="purple">
            管理者管理
          </Title>
          {canAdd && (
            <Button
              type="default"
              icon={<Plus className="size-4" />}
              onClick={() => {
                add.reset();
                setOpen(true);
              }}
            >
              管理者を追加
            </Button>
          )}
        </header>

        <Divider type="line-brown" />

        <p className="text-sm font-medium text-foreground">
          管理者は管理画面の操作(他人の資源の停止 / 削除、共有パスワード、IP 許可リスト)を
          行えます。<strong>最大 {MAX_OWNERS} 名</strong>・自分自身は外せません(最低 1 名必要)。
          外された人にはメールで通知します。
        </p>

        {error && (
          <p className="text-sm font-semibold text-[#e05a5a]">
            読み込みに失敗しました:{error.message}
          </p>
        )}

        {isPending && !owners && (
          <p className="text-sm font-medium text-muted-foreground">読み込み中…</p>
        )}

        {owners && (
          <ul className="flex flex-col gap-3">
            {owners.map((o) => (
              <li key={o.email}>
                <Card>
                  <CardContent className="flex items-center justify-between gap-4">
                    <div className="flex min-w-0 items-center gap-3.5">
                      <div className="grid size-11 shrink-0 place-items-center rounded-2xl bg-accent text-accent-foreground">
                        <ShieldCheck className="size-5.5" />
                      </div>
                      <div className="flex min-w-0 flex-col">
                        <span className="truncate text-base font-bold text-foreground">
                          {o.name ?? o.email}
                          {o.is_current && (
                            <span className="ml-2 text-xs font-bold text-[#0b9c93]">(あなた)</span>
                          )}
                        </span>
                        <span className="truncate text-xs font-medium text-muted-foreground">
                          {o.name ? `${o.email} · ` : ""}
                          {o.registered ? "有効" : "未ログイン(次回ログインで有効)"}
                        </span>
                      </div>
                    </div>
                    {/* 自分は外せない(最低 1 名)。後端でも二重に守る。 */}
                    {!o.is_current && (
                      <Button
                        type="default"
                        size="small"
                        danger
                        icon={<Trash2 className="size-4" />}
                        onClick={() => {
                          remove.reset();
                          setRemoveTarget(o);
                        }}
                      >
                        外す
                      </Button>
                    )}
                  </CardContent>
                </Card>
              </li>
            ))}
          </ul>
        )}

        {/* owner 追加 */}
        <Modal
          open={open}
          title="管理者を追加"
          typewriter={false}
          onClose={() => setOpen(false)}
          width={460}
          footer={
            <>
              <Button type="text" onClick={() => setOpen(false)}>
                キャンセル
              </Button>
              <Button
                type="primary"
                icon={<UserPlus className="size-4" />}
                loading={add.isPending}
                onClick={submit}
              >
                追加
              </Button>
            </>
          }
        >
          <form
            onSubmit={(ev) => {
              ev.preventDefault();
              submit();
            }}
            className="flex w-full flex-col gap-3"
          >
            <Input
              label="メールアドレス(会社ドメイン)"
              type="email"
              placeholder="例:colleague@example.co.jp"
              value={email}
              autoFocus
              onChange={(ev) => setEmail(ev.target.value)}
              description="まだログインしていない人も追加できます(次回ログイン時に管理者になります)。"
            />
            {add.error && (
              <p className="text-sm font-semibold text-[#e05a5a]">{add.error.message}</p>
            )}
          </form>
        </Modal>

        {/* 外す確認 */}
        <Modal
          open={removeTarget !== null}
          title="管理者を外す"
          typewriter={false}
          width={460}
          onClose={() => setRemoveTarget(null)}
          footer={
            <>
              <Button type="text" onClick={() => setRemoveTarget(null)}>
                キャンセル
              </Button>
              <Button
                type="primary"
                danger
                loading={remove.isPending}
                onClick={() => {
                  if (!removeTarget) return;
                  remove.mutate(removeTarget.email, {
                    onSuccess: () => setRemoveTarget(null),
                  });
                }}
              >
                外す
              </Button>
            </>
          }
        >
          <p className="text-sm font-medium text-foreground">
            <strong>{removeTarget?.name ?? removeTarget?.email}</strong> を管理者から外します。
            この人は管理画面を操作できなくなり、メールで通知されます(必要なら後でまた追加できます)。
          </p>
          {removeTarget && (
            <p className="mt-2 flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
              <Users className="size-3.5" />
              {removeTarget.email}
            </p>
          )}
          {remove.error && (
            <p className="mt-2 text-sm font-semibold text-[#e05a5a]">{remove.error.message}</p>
          )}
        </Modal>
      </div>
    </PageContainer>
  );
}
