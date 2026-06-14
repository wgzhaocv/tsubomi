import { useState } from "react";
import { Play, Square, Trash2 } from "lucide-react";
import { useNavigate, useParams } from "react-router";

import { Button } from "@/components/ui/button";
import { Divider } from "@/components/ui/divider";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import {
  shortDigest,
  useDeleteService,
  useService,
  useStartService,
  useStopService,
} from "@/lib/services";

// 概要:状態 grid + 操作(開始 / 停止)+ 危険ゾーン(削除 = 名前入力確認)。
// 操作は再デプロイ(start-first)を伴うので結果が返るまで loading。
export default function ServiceOverview() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const { data: svc } = useService(id);
  const start = useStartService(id);
  const stop = useStopService(id);
  const del = useDeleteService(id);

  const [deleteOpen, setDeleteOpen] = useState(false);
  const [confirmName, setConfirmName] = useState("");
  const actionErr = start.error ?? stop.error;
  // svc 未取得 / どちらかの操作が進行中なら両ボタンを止める(未知状態への発火・start と stop の同時発火を防ぐ)。
  const busy = !svc || start.isPending || stop.isPending;

  return (
    <div className="flex flex-col gap-7">
      {/* ===== 状態 ===== */}
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-bold text-foreground">状態</h2>
        <dl className="grid grid-cols-2 gap-px overflow-hidden rounded-2xl border-2 border-[#e8e2d6] bg-[#e8e2d6] sm:grid-cols-3">
          <Stat label="phase">{svc?.phase ?? "…"}</Stat>
          <Stat label="希望状態">{svc?.desired_state ?? "…"}</Stat>
          <Stat label="ポート">{svc?.container_port ?? "…"}</Stat>
          <Stat label="サブドメイン">{svc?.subdomain ?? "…"}</Stat>
          <Stat label="イメージ">
            {svc?.image_digest ? shortDigest(svc.image_digest) : "未デプロイ"}
          </Stat>
          <Stat label="最終デプロイ">
            {svc?.last_deploy_at ? new Date(svc.last_deploy_at).toLocaleString("ja-JP") : "—"}
          </Stat>
        </dl>
      </section>

      <Divider type="line-brown" />

      {/* ===== 操作 ===== */}
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-bold text-foreground">操作</h2>
        <p className="text-sm font-medium text-muted-foreground">
          停止するとコンテナを止め、ルートを外します。開始は最後に成功したデプロイのイメージで再起動します。
        </p>
        <div className="flex flex-wrap gap-2">
          <Button
            type="primary"
            icon={<Play className="size-4" />}
            loading={start.isPending}
            disabled={busy || svc?.desired_state === "running"}
            onClick={() => start.mutate()}
          >
            開始
          </Button>
          <Button
            type="default"
            icon={<Square className="size-4" />}
            loading={stop.isPending}
            disabled={busy || svc?.desired_state === "stopped"}
            onClick={() => stop.mutate()}
          >
            停止
          </Button>
        </div>
        {actionErr && <p className="text-sm font-semibold text-[#e05a5a]">{actionErr.message}</p>}
      </section>

      <Divider type="line-brown" />

      {/* ===== 危険ゾーン ===== */}
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-bold text-[#c94444]">削除</h2>
        <p className="text-sm font-medium text-muted-foreground">
          削除するとコンテナを止めてゴミ箱に入ります(3 日間は復元可能)。
        </p>
        <Button
          type="default"
          danger
          icon={<Trash2 className="size-4" />}
          className="w-fit"
          onClick={() => {
            setConfirmName("");
            setDeleteOpen(true);
          }}
        >
          このサービスを削除
        </Button>
      </section>

      {/* 削除確認(名前入力) */}
      <Modal
        open={deleteOpen}
        title="サービスを削除"
        typewriter={false}
        width={460}
        onClose={() => setDeleteOpen(false)}
        footer={
          <>
            <Button type="text" onClick={() => setDeleteOpen(false)}>
              キャンセル
            </Button>
            <Button
              type="primary"
              danger
              loading={del.isPending}
              disabled={confirmName !== svc?.display_name}
              onClick={() =>
                del.mutate(undefined, {
                  onSuccess: () => {
                    setDeleteOpen(false);
                    navigate("/services");
                  },
                })
              }
            >
              削除する
            </Button>
          </>
        }
      >
        <div className="flex w-full flex-col gap-3">
          <p>
            確認のため、サービス名 <strong>{svc?.display_name}</strong> を入力してください。
          </p>
          <Input
            value={confirmName}
            autoFocus
            placeholder={svc?.display_name}
            onChange={(e) => setConfirmName(e.target.value)}
          />
          {del.error && <p className="text-sm font-semibold text-[#e05a5a]">{del.error.message}</p>}
        </div>
      </Modal>
    </div>
  );
}

// 状態グリッドの 1 セル(DatabaseOverview と同じ作法)。
function Stat({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1 bg-card px-4 py-3">
      <dt className="text-xs font-semibold text-muted-foreground">{label}</dt>
      <dd className="truncate text-sm font-bold text-foreground">{children}</dd>
    </div>
  );
}
