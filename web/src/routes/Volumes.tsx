import { useState } from "react";
import { HardDrive, Plus } from "lucide-react";
import { useNavigate } from "react-router";

import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Divider } from "@/components/ui/divider";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { Title } from "@/components/ui/title";
import { useCreateVolume, useVolumes } from "@/lib/volumes";

// ボリューム一覧。RESOURCES(サイドメニュー)の「ボリューム」項目に対応する実画面。
// 作成は名前を 1 つ入れるだけ(平台が一意な host_path を生成する)。クリックで
// ファイルブラウザ(/volumes/:id/files)へ — 假根の中をそのまま URL に持つ。

export default function Volumes() {
  const navigate = useNavigate();
  const { data: volumes, isPending, error } = useVolumes();
  const create = useCreateVolume();

  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");

  const submit = () => {
    const trimmed = name.trim();
    if (!trimmed || create.isPending) return; // 二重送信を防ぐ(連打 / Enter+クリック)
    create.mutate(trimmed, {
      onSuccess: (vol) => {
        setOpen(false);
        setName("");
        navigate(`/volumes/${vol.id}/files`);
      },
    });
  };

  return (
    <PageContainer>
      <div className="flex flex-col gap-7">
        <PageMeta title="ボリューム" />

        <header className="flex flex-wrap items-center justify-between gap-4">
          <Title size="large" color="app-yellow">
            ボリューム
          </Title>
          {/* 空のときは下の空状態 CTA に任せ、1 つ以上あるときだけ右上に出す。 */}
          {volumes && volumes.length > 0 && (
            <Button type="default" icon={<Plus className="size-4" />} onClick={() => setOpen(true)}>
              ボリュームを作成
            </Button>
          )}
        </header>

        <Divider type="line-brown" />

        {error && (
          <p className="text-sm font-semibold text-[#e05a5a]">
            読み込みに失敗しました:{error.message}
          </p>
        )}

        {!isPending && volumes && volumes.length === 0 && (
          <Card type="dashed">
            <CardContent className="flex flex-col items-center gap-4 px-6 py-12 text-center">
              <div className="grid size-16 place-items-center rounded-full bg-accent text-accent-foreground">
                <HardDrive className="size-8" />
              </div>
              <div className="flex flex-col gap-1.5">
                <p className="text-lg font-bold text-foreground">まだボリュームがありません</p>
                <p className="max-w-md text-sm font-medium text-muted-foreground">
                  ファイルを置く永続ディスク領域です。web と CLI
                  からファイルを作成・削除でき、サービスに注入(M3)して使えます。
                </p>
              </div>
              <Button
                type="primary"
                icon={<Plus className="size-4" />}
                onClick={() => setOpen(true)}
              >
                ボリュームを作成
              </Button>
            </CardContent>
          </Card>
        )}

        {volumes && volumes.length > 0 && (
          <ul className="flex flex-col gap-3">
            {volumes.map((vol) => (
              <li key={vol.id}>
                <Card
                  interactive
                  onClick={() => navigate(`/volumes/${vol.id}/files`)}
                  className="flex-row items-center justify-between gap-4 py-4"
                >
                  <CardContent className="flex min-w-0 items-center gap-3.5">
                    <div className="grid size-11 shrink-0 place-items-center rounded-2xl bg-accent text-accent-foreground">
                      <HardDrive className="size-5.5" />
                    </div>
                    <div className="flex min-w-0 flex-col">
                      <span className="truncate text-base font-bold text-foreground">
                        {vol.display_name}
                      </span>
                      <span className="truncate text-xs font-medium text-muted-foreground">
                        volume{vol.anon_seq} · 作成{" "}
                        {new Date(vol.created_at).toLocaleDateString("ja-JP")}
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
          title="ボリュームを作成"
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
          {/* 本物の form。Enter は onSubmit を 1 回だけ通す(独自 onKeyDown は廃止)。 */}
          <form
            onSubmit={(e) => {
              e.preventDefault();
              submit();
            }}
            className="flex w-full flex-col gap-3"
          >
            <Input
              label="名前"
              placeholder="例:myapp-storage"
              value={name}
              autoFocus
              onChange={(e) => setName(e.target.value)}
              description="表示名です。後から変えても保存したファイルは変わりません。"
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
