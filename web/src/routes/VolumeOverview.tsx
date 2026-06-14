import { useState } from "react";
import { FolderOpen, Trash2 } from "lucide-react";
import { useNavigate, useParams } from "react-router";

import { Button } from "@/components/ui/button";
import { Divider } from "@/components/ui/divider";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { formatBytes, useDeleteVolume, useVolumeUsage, useVolumes } from "@/lib/volumes";

// 概要ページ:状態(メタデータ)+ ファイルブラウザへの導線 + 危険ゾーン(削除)。
// 注入(M3)が入るまではファイル置き場の単体運用。DatabaseOverview の簡略版。

export default function VolumeOverview() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const { data: volumes } = useVolumes();
  const vol = volumes?.find((v) => v.id === id);
  const { data: usage } = useVolumeUsage(id);
  const del = useDeleteVolume();

  const [deleteOpen, setDeleteOpen] = useState(false);
  const [confirmName, setConfirmName] = useState("");

  return (
    <div className="flex flex-col gap-7">
      {/* ===== 状態 ===== */}
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-bold text-foreground">状態</h2>
        <dl className="grid grid-cols-2 gap-px overflow-hidden rounded-2xl border-2 border-[#e8e2d6] bg-[#e8e2d6] sm:grid-cols-4">
          <Stat label="状態">
            <span className="inline-flex items-center gap-1.5 font-bold text-[#11a89b]">
              <span className="size-2 rounded-full bg-[#19c8b9]" />
              利用可能
            </span>
          </Stat>
          <Stat label="合計サイズ">{usage ? formatBytes(usage.size_bytes) : "…"}</Stat>
          <Stat label="ファイル数">{usage ? usage.file_count.toLocaleString("ja-JP") : "…"}</Stat>
          <Stat label="作成日">
            {vol ? new Date(vol.created_at).toLocaleDateString("ja-JP") : "…"}
          </Stat>
        </dl>
        <Button
          type="default"
          icon={<FolderOpen className="size-4" />}
          className="w-fit"
          onClick={() => navigate(`/volumes/${id}/files`)}
        >
          ファイルを開く
        </Button>
      </section>

      <Divider type="line-brown" />

      {/* ===== 危険ゾーン ===== */}
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-bold text-[#c94444]">削除</h2>
        <p className="text-sm font-medium text-muted-foreground">
          削除するとゴミ箱に入ります(3 日間は復元可能)。中のファイルごと退避されます。
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
          このボリュームを削除
        </Button>
      </section>

      {/* 削除確認(名前入力) */}
      <Modal
        open={deleteOpen}
        title="ボリュームを削除"
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
              disabled={confirmName !== vol?.display_name}
              onClick={() =>
                del.mutate(id, {
                  onSuccess: () => {
                    setDeleteOpen(false);
                    navigate("/volumes");
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
            確認のため、ボリューム名 <strong>{vol?.display_name}</strong> を入力してください。
          </p>
          <Input
            value={confirmName}
            autoFocus
            placeholder={vol?.display_name}
            onChange={(e) => setConfirmName(e.target.value)}
          />
          {del.error && <p className="text-sm font-semibold text-[#e05a5a]">{del.error.message}</p>}
        </div>
      </Modal>
    </div>
  );
}

// 状態グリッドの 1 セル(ラベル + 値)。DatabaseOverview と同じ。
function Stat({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1 bg-card px-4 py-3">
      <dt className="text-xs font-semibold text-muted-foreground">{label}</dt>
      <dd className="text-sm font-bold text-foreground">{children}</dd>
    </div>
  );
}
