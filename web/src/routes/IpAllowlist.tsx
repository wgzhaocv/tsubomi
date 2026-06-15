import { useState } from "react";
import { Globe, Plus, ShieldCheck, Trash2 } from "lucide-react";

import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Divider } from "@/components/ui/divider";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { Title } from "@/components/ui/title";
import { useMeQuery } from "@/lib/auth";
import { type IpAllowEntry, useAddIpAllow, useIpAllowlist, useRemoveIpAllow } from "@/lib/ipblock";

// 会社 IP 許可リスト(owner 専用)。traefik の ipAllowList へ流す CIDR の管制面。
//   * 空        = 制限なし(全 IP 許可)。設定するまで誰でも service に繋がる。
//   * 1 件以上  = 列挙した CIDR だけが service に到達でき、他は遮断。
// 反映はライブ(平台が traefik の動的設定を書き直す)。ただし古いコンテナは
// 一度 redeploy して初めて許可リストの対象になる(新規 deploy は最初から対象)。

export default function IpAllowlist() {
  const { data: me } = useMeQuery();
  const { data: entries, isPending, error } = useIpAllowlist();
  const add = useAddIpAllow();
  const remove = useRemoveIpAllow();

  const [open, setOpen] = useState(false);
  const [cidr, setCidr] = useState("");
  const [note, setNote] = useState("");
  const [removeTarget, setRemoveTarget] = useState<IpAllowEntry | null>(null);

  // owner 以外はサイドメニューに出さないが、URL 直打ち / 降格に備えて画面でも弾く
  // (バックエンドも 403 で守る — フロントの表示制御はただの UX)。
  if (me && me.role !== "owner") {
    return (
      <PageContainer>
        <div className="flex flex-col gap-7">
          <PageMeta title="IP 許可リスト" />
          <Title size="large" color="purple">
            IP 許可リスト
          </Title>
          <Divider type="line-brown" />
          <p className="text-sm font-semibold text-[#e05a5a]">
            この画面は管理者だけが利用できます。
          </p>
        </div>
      </PageContainer>
    );
  }

  const submit = () => {
    const trimmed = cidr.trim();
    if (!trimmed || add.isPending) return; // 二重送信を防ぐ
    add.mutate(
      { cidr: trimmed, note: note.trim() },
      {
        onSuccess: () => {
          setOpen(false);
          setCidr("");
          setNote("");
        },
      },
    );
  };

  const isEmpty = !isPending && entries && entries.length === 0;
  const hasEntries = entries && entries.length > 0;

  return (
    <PageContainer>
      <div className="flex flex-col gap-7">
        <PageMeta title="IP 許可リスト" />

        <header className="flex flex-wrap items-center justify-between gap-4">
          <Title size="large" color="purple">
            IP 許可リスト
          </Title>
          {hasEntries && (
            <Button type="default" icon={<Plus className="size-4" />} onClick={() => setOpen(true)}>
              レンジを追加
            </Button>
          )}
        </header>

        <Divider type="line-brown" />

        {/* 現在の状態バナー:空 = 全許可 / 1 件以上 = 制限中。意味が大きく変わるので強調する。 */}
        {hasEntries ? (
          <div className="flex items-start gap-3 rounded-2xl border-2 border-[#0CC0B5] bg-[rgba(12,192,181,0.08)] px-4 py-3">
            <ShieldCheck className="mt-0.5 size-5 shrink-0 text-[#0b9c93]" />
            <p className="text-sm font-medium text-foreground">
              <strong>制限中。</strong>下に登録した {entries.length}{" "}
              件の範囲からのみサービスに到達できます。それ以外の IP は遮断されます。
            </p>
          </div>
        ) : (
          <div className="flex items-start gap-3 rounded-2xl border-2 border-[#e8b94a] bg-[rgba(232,185,74,0.12)] px-4 py-3">
            <Globe className="mt-0.5 size-5 shrink-0 text-[#b8860b]" />
            <p className="text-sm font-medium text-foreground">
              <strong>現在は全ての IP からアクセスできます</strong>(許可リストが空 =
              制限なし)。範囲を 1 件でも追加すると、その範囲だけに制限されます。
            </p>
          </div>
        )}

        {error && (
          <p className="text-sm font-semibold text-[#e05a5a]">
            読み込みに失敗しました:{error.message}
          </p>
        )}
        {remove.error && (
          <p className="text-sm font-semibold text-[#e05a5a]">{remove.error.message}</p>
        )}

        {isEmpty && (
          <Card type="dashed">
            <CardContent className="flex flex-col items-center gap-4 px-6 py-12 text-center">
              <div className="grid size-16 place-items-center rounded-full bg-accent text-accent-foreground">
                <ShieldCheck className="size-8" />
              </div>
              <div className="flex flex-col gap-1.5">
                <p className="text-lg font-bold text-foreground">まだ許可レンジがありません</p>
                <p className="max-w-md text-sm font-medium text-muted-foreground">
                  会社のオフィスや VPN の IP レンジ(CIDR)を登録すると、サービスへの
                  アクセスをその範囲だけに制限できます。registry とデプロイ用 hook は対象外です。
                </p>
              </div>
              <Button
                type="primary"
                icon={<Plus className="size-4" />}
                onClick={() => setOpen(true)}
              >
                レンジを追加
              </Button>
            </CardContent>
          </Card>
        )}

        {hasEntries && (
          <ul className="flex flex-col gap-3">
            {entries.map((e) => (
              <li key={e.id}>
                <Card>
                  <CardContent className="flex items-center justify-between gap-4">
                    <div className="flex min-w-0 items-center gap-3.5">
                      <div className="grid size-11 shrink-0 place-items-center rounded-2xl bg-accent text-accent-foreground">
                        <ShieldCheck className="size-5.5" />
                      </div>
                      <div className="flex min-w-0 flex-col">
                        <span className="truncate font-mono text-base font-bold text-foreground">
                          {e.cidr}
                        </span>
                        <span className="truncate text-xs font-medium text-muted-foreground">
                          {e.note ? `${e.note} · ` : ""}追加{" "}
                          {new Date(e.created_at).toLocaleDateString("ja-JP")}
                        </span>
                      </div>
                    </div>
                    <Button
                      type="default"
                      size="small"
                      danger
                      icon={<Trash2 className="size-4" />}
                      onClick={() => setRemoveTarget(e)}
                    >
                      削除
                    </Button>
                  </CardContent>
                </Card>
              </li>
            ))}
          </ul>
        )}

        {/* レンジ追加 */}
        <Modal
          open={open}
          title="レンジを追加"
          typewriter={false}
          onClose={() => setOpen(false)}
          width={460}
          footer={
            <>
              <Button type="text" onClick={() => setOpen(false)}>
                キャンセル
              </Button>
              <Button type="primary" loading={add.isPending} onClick={submit}>
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
              label="IP レンジ(CIDR)"
              placeholder="例:203.0.113.0/24 または 198.51.100.7"
              value={cidr}
              autoFocus
              onChange={(ev) => setCidr(ev.target.value)}
              description="単一 IP も指定できます(/32 として扱います)。IPv6 も可。"
            />
            <Input
              label="メモ(任意)"
              placeholder="例:東京オフィス"
              value={note}
              onChange={(ev) => setNote(ev.target.value)}
              description="何の IP かを後で分かるように。"
            />
            {add.error && (
              <p className="text-sm font-semibold text-[#e05a5a]">{add.error.message}</p>
            )}
          </form>
        </Modal>

        {/* 削除の確認 */}
        <Modal
          open={removeTarget !== null}
          title="レンジを削除"
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
                  remove.mutate(removeTarget.id, { onSuccess: () => setRemoveTarget(null) });
                }}
              >
                削除する
              </Button>
            </>
          }
        >
          <p>
            <strong className="font-mono">{removeTarget?.cidr}</strong> を許可リストから削除します。
            {entries?.length === 1 && (
              <>
                {" "}
                これが最後の 1 件です。削除すると<strong>全ての IP が再び許可</strong>
                されます。
              </>
            )}
          </p>
        </Modal>
      </div>
    </PageContainer>
  );
}
