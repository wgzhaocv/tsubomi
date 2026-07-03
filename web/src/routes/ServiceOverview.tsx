import { useState } from "react";
import { Check, Copy, ExternalLink, EyeOff, Globe, Play, Square, Trash2 } from "lucide-react";
import { useNavigate, useParams } from "react-router";

import { Button } from "@/components/ui/button";
import { Divider } from "@/components/ui/divider";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { Radio } from "@/components/ui/radio";
import {
  desiredLabel,
  phaseLabel,
  serviceVisibility,
  shortDigest,
  useDeleteService,
  useService,
  useSetServiceVisibility,
  useStartService,
  useStopService,
  VISIBILITY_OPTIONS,
} from "@/lib/services";
import { useCopied } from "@/lib/use-copied";
import { cn } from "@/lib/utils";

// 概要:状態 grid + 操作(開始 / 停止)+ 公開範囲(Radio 3 択)+ 危険ゾーン(削除 = 名前入力確認)。
// 操作は再デプロイ(start-first)を伴うので結果が返るまで loading。
export default function ServiceOverview() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const { data: svc } = useService(id);
  const start = useStartService(id);
  const stop = useStopService(id);
  const del = useDeleteService(id);
  const setVis = useSetServiceVisibility(id);

  const [deleteOpen, setDeleteOpen] = useState(false);
  const [confirmName, setConfirmName] = useState("");
  const { copied, copy } = useCopied();
  // url を局所定数に取り出して narrow する(onClick クロージャ内でも string 確定にする)。
  const url = svc?.url;
  const urlText = url?.replace(/^https?:\/\//, "");
  const visibility = serviceVisibility(svc);
  const isPrivate = visibility === "private";
  const actionErr = start.error ?? stop.error;
  // svc 未取得 / どちらかの操作が進行中なら両ボタンを止める(未知状態への発火・start と stop の同時発火を防ぐ)。
  const busy = !svc || start.isPending || stop.isPending;

  return (
    <div className="flex flex-col gap-7">
      {/* ===== 公開 URL(目立つ位置に独立表示。クリックで開く / コピー)=====
          private 中は**消さずに灰色化**して「非公開中」を明示 — subdomain は温存されており、
          再公開すれば同じ URL で復活するため。URL 文字列とコピーは残し、「開く」は /noservice に
          飛ぶだけなので出さない。 */}
      {url && (
        <section
          className={cn(
            "flex flex-wrap items-center gap-3 rounded-2xl border-2 px-5 py-4",
            isPrivate ? "border-[#e8e2d6] bg-card" : "border-[#19c8b9]/35 bg-accent",
          )}
        >
          <div
            className={cn(
              "grid size-11 shrink-0 place-items-center rounded-2xl",
              isPrivate
                ? "bg-[#e8e2d6]/60 text-muted-foreground"
                : "bg-[#19c8b9]/15 text-[#11a89b]",
            )}
          >
            {isPrivate ? <EyeOff className="size-5.5" /> : <Globe className="size-5.5" />}
          </div>
          <div className="flex min-w-0 flex-1 flex-col">
            <span className="text-xs font-bold text-muted-foreground">
              {isPrivate ? "公開 URL(非公開中 — 外部からはアクセスできません)" : "公開 URL"}
            </span>
            {isPrivate ? (
              <span className="truncate text-base font-bold text-muted-foreground">{urlText}</span>
            ) : (
              <a
                href={url}
                target="_blank"
                rel="noreferrer"
                className="truncate text-base font-bold text-[#11a89b] underline-offset-2 outline-none hover:underline focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-2"
              >
                {urlText}
              </a>
            )}
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <Button
              type="default"
              size="small"
              icon={copied ? <Check className="size-4" /> : <Copy className="size-4" />}
              onClick={() => copy(url)}
            >
              {copied ? "コピー済み" : "コピー"}
            </Button>
            {!isPrivate && (
              <Button type="primary" size="small" asChild>
                <a href={url} target="_blank" rel="noreferrer">
                  <ExternalLink className="size-4" />
                  開く
                </a>
              </Button>
            )}
          </div>
        </section>
      )}

      {/* ===== 状態 ===== */}
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-bold text-foreground">状態</h2>
        <dl className="grid grid-cols-2 gap-px overflow-hidden rounded-2xl border-2 border-[#e8e2d6] bg-[#e8e2d6] sm:grid-cols-3">
          <Stat label="現在の状態">{svc ? phaseLabel(svc.phase) : "…"}</Stat>
          <Stat label="希望状態">{svc ? desiredLabel(svc.desired_state) : "…"}</Stat>
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

      {/* ===== 公開範囲(即時反映・再デプロイ不要)===== */}
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-bold text-foreground">公開範囲</h2>
        <p className="text-sm font-medium text-muted-foreground">
          切り替えは即時反映(再デプロイ不要)。非公開にしても内部リンク・ログ・ターミナルは従来どおり使えます。一般公開は
          IP 制限が外れます — アプリ側の認証にご注意ください。
        </p>
        <Radio
          aria-label="公開範囲"
          value={visibility}
          disabled={!svc || setVis.isPending}
          options={[...VISIBILITY_OPTIONS]}
          onChange={(v) => setVis.mutate(String(v))}
        />
        {setVis.error && (
          <p className="text-sm font-semibold text-[#e05a5a]">{setVis.error.message}</p>
        )}
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
