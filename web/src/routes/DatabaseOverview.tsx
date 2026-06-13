import { useState } from "react";
import { Eye, EyeOff, RotateCw, Trash2, TriangleAlert } from "lucide-react";
import { useNavigate, useParams } from "react-router";

import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/codeblock";
import { Divider } from "@/components/ui/divider";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import {
  useDatabases,
  useDeleteDatabase,
  useRevealUrl,
  useRotate,
  useTables,
} from "@/lib/databases";

// 概要ページ:状態(メタデータ)+ 接続文字列(表示 / rotate)+ 危険ゾーン(削除)。
// 接続文字列は秘密なので既定は隠し、表示要求時だけ取得して画面ローカルに置く。

export default function DatabaseOverview() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const { data: dbs } = useDatabases();
  const db = dbs?.find((d) => d.id === id);
  const { data: tables } = useTables(id);

  const reveal = useRevealUrl();
  const rotate = useRotate();
  const del = useDeleteDatabase();

  // 表示中の接続文字列(reveal / rotate が入れる)。null = 隠している。
  const [url, setUrl] = useState<string | null>(null);
  const [rotateOpen, setRotateOpen] = useState(false);
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
              稼働中
            </span>
          </Stat>
          <Stat label="テーブル数">{tables ? `${tables.length}` : "…"}</Stat>
          <Stat label="作成日">
            {db ? new Date(db.created_at).toLocaleDateString("ja-JP") : "…"}
          </Stat>
          <Stat label="最終 rotate">
            {db?.rotated_at ? new Date(db.rotated_at).toLocaleDateString("ja-JP") : "—"}
          </Stat>
        </dl>
      </section>

      <Divider type="line-brown" />

      {/* ===== 接続文字列 ===== */}
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-bold text-foreground">接続文字列</h2>
        <div className="flex items-start gap-2 rounded-2xl border-2 border-[#f5c31c] bg-[rgba(245,195,28,0.1)] px-4 py-3">
          <TriangleAlert className="mt-0.5 size-4.5 shrink-0 text-[#dba90e]" />
          <p className="text-sm font-semibold text-[#8a6d12]">
            この文字列は<strong>パスワードそのもの</strong>です。git に commit
            したり、人に共有したりしないでください。漏れたら rotate で失効できます。
          </p>
        </div>

        {url ? (
          <div className="flex flex-col gap-2">
            <CodeBlock code={url} language="postgres" showCopy />
            <div className="flex flex-wrap gap-2">
              <Button
                type="text"
                size="small"
                icon={<EyeOff className="size-4" />}
                onClick={() => setUrl(null)}
              >
                隠す
              </Button>
              <Button
                type="default"
                size="small"
                danger
                icon={<RotateCw className="size-4" />}
                onClick={() => setRotateOpen(true)}
              >
                rotate(再生成)
              </Button>
            </div>
          </div>
        ) : (
          <div className="flex flex-wrap gap-2">
            <Button
              type="primary"
              icon={<Eye className="size-4" />}
              loading={reveal.isPending}
              onClick={() => reveal.mutate(id, { onSuccess: setUrl })}
            >
              接続文字列を表示
            </Button>
            <Button
              type="default"
              danger
              icon={<RotateCw className="size-4" />}
              onClick={() => setRotateOpen(true)}
            >
              rotate(再生成)
            </Button>
          </div>
        )}
        {reveal.error && (
          <p className="text-sm font-semibold text-[#e05a5a]">{reveal.error.message}</p>
        )}
        {db?.rotated_at && (
          <p className="text-xs font-medium text-muted-foreground">
            最終 rotate:{new Date(db.rotated_at).toLocaleString("ja-JP")}
            (これより前にコピーした文字列は失効しています)
          </p>
        )}
      </section>

      <Divider type="line-brown" />

      {/* ===== 危険ゾーン ===== */}
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-bold text-[#c94444]">削除</h2>
        <p className="text-sm font-medium text-muted-foreground">
          削除するとゴミ箱に入ります(3 日間は復元可能)。
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
          このデータベースを削除
        </Button>
      </section>

      {/* rotate 確認 */}
      <Modal
        open={rotateOpen}
        title="接続文字列を rotate"
        typewriter={false}
        width={460}
        onClose={() => setRotateOpen(false)}
        footer={
          <>
            <Button type="text" onClick={() => setRotateOpen(false)}>
              キャンセル
            </Button>
            <Button
              type="primary"
              danger
              loading={rotate.isPending}
              onClick={() =>
                rotate.mutate(id, {
                  onSuccess: (newUrl) => {
                    setUrl(newUrl);
                    setRotateOpen(false);
                  },
                })
              }
            >
              rotate する
            </Button>
          </>
        }
      >
        <p>
          新しいパスワードを発行し、<strong>古い接続文字列は即座に失効</strong>
          します。注入済みのサービスは再デプロイするまで古い文字列のままです。続けますか?
        </p>
      </Modal>

      {/* 削除確認(名前入力) */}
      <Modal
        open={deleteOpen}
        title="データベースを削除"
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
              disabled={confirmName !== db?.display_name}
              onClick={() =>
                del.mutate(id, {
                  onSuccess: () => {
                    setDeleteOpen(false);
                    void navigate("/databases");
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
            確認のため、データベース名 <strong>{db?.display_name}</strong> を入力してください。
          </p>
          <Input
            value={confirmName}
            autoFocus
            placeholder={db?.display_name}
            onChange={(e) => setConfirmName(e.target.value)}
          />
          {del.error && <p className="text-sm font-semibold text-[#e05a5a]">{del.error.message}</p>}
        </div>
      </Modal>
    </div>
  );
}

// 状態グリッドの 1 セル(ラベル + 値)。クリーム面、罫線は親の gap-px が描く。
function Stat({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1 bg-card px-4 py-3">
      <dt className="text-xs font-semibold text-muted-foreground">{label}</dt>
      <dd className="text-sm font-bold text-foreground">{children}</dd>
    </div>
  );
}
