import { useState } from "react";
import { Eye, EyeOff, GitFork, RotateCw, Trash2, TriangleAlert } from "lucide-react";
import { useNavigate, useParams } from "react-router";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { CodeBlock } from "@/components/ui/codeblock";
import { Divider } from "@/components/ui/divider";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { Stat } from "@/components/ui/stat";
import { useAuthInfoQuery } from "@/lib/auth";
import {
  useDatabaseCapacity,
  useDatabases,
  useDeleteDatabase,
  useForkDatabase,
  useRevealUrl,
  useRotate,
  useTables,
} from "@/lib/databases";

// 概要ページ:状態(メタデータ)+ 接続文字列(表示 / rotate)+ 複製 + 危険ゾーン(削除)。
// 接続文字列は秘密なので既定は隠し、表示要求時だけ取得して画面ローカルに置く。

// 現在の DB(URL の :id)を一覧キャッシュから引く。各 section が同じ導出を持っていたのを一本化
// (サーバ状態は各 section が自前で引く方針のまま — これはその共通の入口)。
function useCurrentDb() {
  const { id = "" } = useParams();
  const { data: dbs } = useDatabases();
  return { id, db: dbs?.find((d) => d.id === id) };
}

export default function DatabaseOverview() {
  const navigate = useNavigate();
  const { id, db } = useCurrentDb();
  const { data: tables } = useTables(id);
  // 外部接続文字列機能が有効か(部署のトポロジ依存)。off の環境では接続文字列カードを隠す
  // (防御はバックエンド — ここは UX)。authInfo はアプリ起動時に取得済みでほぼキャッシュ命中。
  const { data: authInfo } = useAuthInfoQuery();

  const del = useDeleteDatabase();

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
          <Stat label="テーブル数">{tables?.length ?? "…"}</Stat>
          <Stat label="作成日">
            {db ? new Date(db.created_at).toLocaleDateString("ja-JP") : "…"}
          </Stat>
          <Stat label="最終 rotate">
            {db?.rotated_at ? new Date(db.rotated_at).toLocaleDateString("ja-JP") : "—"}
          </Stat>
        </dl>
      </section>

      <Divider type="line-brown" />

      {/* ===== 接続容量 ===== */}
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-bold text-foreground">接続容量</h2>
        <CapacitySection />
      </section>

      <Divider type="line-brown" />

      {/* ===== 接続文字列 ===== */}
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-bold text-foreground">接続文字列</h2>
        {authInfo?.db_public_enabled ? (
          <ConnectionStringSection />
        ) : authInfo ? (
          <p className="text-sm font-medium text-muted-foreground">
            この環境では外部からの直接接続は無効です(管理者設定)。データの確認・編集は上部の
            <strong>「SQL」</strong>・<strong>「テーブル」</strong>タブを使ってください。
          </p>
        ) : null}
      </section>

      <Divider type="line-brown" />

      {/* ===== 複製 ===== */}
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-bold text-foreground">複製</h2>
        <ForkSection />
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
                    navigate("/databases");
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

// 複製(fork)カード:この瞬間の構造 + データごと新しい DB を作る。dev/検証環境用の
// 真実データが一発で手に入る。fork 後の同期はしない(分岐した瞬間から別々の道)。
// サーバ状態は自前のフックで引く(props で配らない方針 — [[frontend-state-and-components]])。
function ForkSection() {
  const navigate = useNavigate();
  const { id, db } = useCurrentDb();
  const fork = useForkDatabase(id);

  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [schemaOnly, setSchemaOnly] = useState(false);

  const submit = () => {
    const trimmed = name.trim();
    if (!trimmed || fork.isPending) return;
    fork.mutate(
      { name: trimmed, schemaOnly },
      {
        onSuccess: (newDb) => {
          // `/databases/:id` は id が変わっても同じ route コンポーネント = 再マウントされない
          // ので、遷移前に Modal を明示的に閉じる(閉じないと新 DB のページに複製ダイアログが
          // 開いたまま残る)。遷移が成功のフィードバック(toast は無い文化)。
          setOpen(false);
          navigate(`/databases/${newDb.id}`);
        },
      },
    );
  };

  return (
    <>
      <p className="text-sm font-medium text-muted-foreground">
        この瞬間の内容ごと新しいデータベースを作ります(開発・検証環境用)。複製後に同期は
        されず、それぞれ独立して変化します。新しい DB の接続文字列は元とは別物です。
      </p>
      <Button
        type="default"
        icon={<GitFork className="size-4" />}
        className="w-fit"
        onClick={() => {
          // 実行中に再度開いたときは state を作り直さない(reset すると isPending の観察が
          // 外れて二重 fork を許してしまう)。開き直して進行中の表示に戻すだけ。
          if (!fork.isPending) {
            setName(db ? `${db.display_name}-dev` : "");
            setSchemaOnly(false);
            fork.reset();
          }
          setOpen(true);
        }}
      >
        このデータベースを複製
      </Button>

      <Modal
        open={open}
        title="データベースを複製"
        typewriter={false}
        width={460}
        onClose={() => {
          // 実行中は閉じさせない(閉じても fork は止まらず、完了時に突然遷移して見える)。
          if (!fork.isPending) setOpen(false);
        }}
        footer={
          <>
            <Button type="text" disabled={fork.isPending} onClick={() => setOpen(false)}>
              キャンセル
            </Button>
            <Button
              type="primary"
              loading={fork.isPending}
              disabled={!name.trim()}
              onClick={submit}
            >
              複製する
            </Button>
          </>
        }
      >
        <form
          className="flex w-full flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            submit();
          }}
        >
          <Input
            label="新しい名前"
            value={name}
            autoFocus
            placeholder={`例:${db?.display_name ?? "myapp-db"}-dev`}
            onChange={(e) => setName(e.target.value)}
          />
          <Checkbox
            aria-label="複製の範囲"
            options={[{ label: "スキーマのみ(データを含めない)", value: "schema" }]}
            value={schemaOnly ? ["schema"] : []}
            onChange={(vals) => setSchemaOnly(vals.includes("schema"))}
          />
          <p className="text-sm font-medium text-muted-foreground">
            データ量によっては完了まで時間がかかります。
          </p>
          {fork.error && (
            <p className="text-sm font-semibold text-[#e05a5a]">{fork.error.message}</p>
          )}
        </form>
      </Modal>
    </>
  );
}

// 接続容量カード:1 ロールあたりの上限 + 実時の使用量(human / app)。「接続を食い潰して
// いないか」を可視化する。実時用量は /capacity を定期取得(useDatabaseCapacity が 15s 毎)。
function CapacitySection() {
  const { id = "" } = useParams();
  const { data: cap } = useDatabaseCapacity(id);
  return (
    <>
      <dl className="grid grid-cols-2 gap-px overflow-hidden rounded-2xl border-2 border-[#e8e2d6] bg-[#e8e2d6] sm:grid-cols-3">
        <Stat label="接続上限">{cap ? cap.conn_limit : "…"}</Stat>
        <Stat label="現在アクティブ(human)">{cap ? cap.human_connections : "…"}</Stat>
        <Stat label="現在アクティブ(app)">{cap ? cap.app_connections : "…"}</Stat>
      </dl>
      <p className="text-sm font-medium text-muted-foreground">
        最大 {cap?.conn_limit ?? "…"} 本まで接続できます。コネクションプールの利用を推奨します
        (リクエスト毎に新規接続せず、少数の長命接続を使い回す)。pgbouncer が{" "}
        {cap?.pool_mode ?? "transaction"}{" "}
        プールで多重化するため、「現在アクティブ」は今クエリを実行中の接続数で、上限よりかなり小さく保たれます。
      </p>
    </>
  );
}

// 接続文字列カード(表示 / rotate + rotate 確認モーダル)。外部接続が有効な部署でのみ親が描画する。
// サーバ状態は自前のフックで引く(props で配らない方針 — [[frontend-state-and-components]])。
// 秘密の接続文字列は Query に載せず、表示 / rotate 要求時だけ取得して画面ローカルに置く。
function ConnectionStringSection() {
  const { id, db } = useCurrentDb();
  const reveal = useRevealUrl();
  const rotate = useRotate();

  // 表示中の接続文字列(reveal / rotate が入れる)。null = 隠している。
  const [url, setUrl] = useState<string | null>(null);
  const [rotateOpen, setRotateOpen] = useState(false);

  return (
    <>
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
    </>
  );
}
