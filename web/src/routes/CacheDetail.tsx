import { useState } from "react";
import { ArrowLeft, Eye, EyeOff, RotateCw, Trash2, TriangleAlert } from "lucide-react";
import { Link, useNavigate, useParams } from "react-router";

import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/codeblock";
import { Divider } from "@/components/ui/divider";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { Stat } from "@/components/ui/stat";
import { Title } from "@/components/ui/title";
import {
  useCache,
  useDeleteCache,
  useRenameCache,
  useRevealUrl,
  useRotate,
} from "@/lib/caches";

// キャッシュ詳細(単一ページ):戻りリンク + 見出し(+ リネーム)+ 状態 + 接続文字列
// (表示 / rotate)+ 危険ゾーン(削除)。cache はタブが概要のみなので Layout/Outlet は使わない。
// 接続文字列(REDIS_URL)は秘密かつ**内部入口**(注入された service コンテナからのみ繋がる)。

export default function CacheDetail() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const { data: cache, error } = useCache(id);

  const rename = useRenameCache(id);
  const reveal = useRevealUrl();
  const rotate = useRotate();
  const del = useDeleteCache();

  // 表示中の接続文字列(reveal / rotate が入れる)。null = 隠している。
  const [url, setUrl] = useState<string | null>(null);
  const [renameOpen, setRenameOpen] = useState(false);
  const [renameName, setRenameName] = useState("");
  const [rotateOpen, setRotateOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [confirmName, setConfirmName] = useState("");

  const submitRename = () => {
    const trimmed = renameName.trim();
    if (!trimmed || rename.isPending) return;
    rename.mutate(trimmed, { onSuccess: () => setRenameOpen(false) });
  };

  return (
    <PageContainer>
      <div className="flex flex-col gap-6">
        <PageMeta title={cache ? cache.display_name : "キャッシュ"} />

        <div className="flex flex-col gap-3">
          <Link
            to="/caches"
            className="inline-flex w-fit items-center gap-1.5 text-sm font-semibold text-muted-foreground outline-none hover:text-[#11a89b] focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-2"
          >
            <ArrowLeft className="size-4" />
            キャッシュ一覧へ
          </Link>
          <header className="flex flex-wrap items-center justify-between gap-4">
            {cache ? (
              <button
                type="button"
                aria-label="キャッシュ名を変更"
                title="クリックして名前を変更"
                onClick={() => {
                  setRenameName(cache.display_name);
                  setRenameOpen(true);
                }}
                className="group w-fit cursor-pointer rounded-2xl outline-none focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-4"
              >
                <Title size="large" color="app-orange">
                  {cache.display_name}
                </Title>
              </button>
            ) : (
              <Title size="large" color="app-orange">
                {id}
              </Title>
            )}
            {cache && (
              <span className="rounded-full bg-accent px-3 py-1 text-xs font-bold text-accent-foreground">
                cache{cache.anon_seq}
              </span>
            )}
          </header>
        </div>

        <Divider type="line-brown" />

        {error && (
          <p className="text-sm font-semibold text-[#e05a5a]">
            読み込みに失敗しました:{error.message}
          </p>
        )}

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
            <Stat label="キー数(概算)">{cache ? (cache.key_count ?? "—") : "…"}</Stat>
            <Stat label="作成日">
              {cache ? new Date(cache.created_at).toLocaleDateString("ja-JP") : "…"}
            </Stat>
            <Stat label="最終 rotate">
              {cache?.rotated_at ? new Date(cache.rotated_at).toLocaleDateString("ja-JP") : "—"}
            </Stat>
          </dl>
        </section>

        <Divider type="line-brown" />

        {/* ===== 接続(注入)情報 ===== */}
        <section className="flex flex-col gap-3">
          <h2 className="text-lg font-bold text-foreground">接続文字列</h2>
          <div className="flex items-start gap-2 rounded-2xl border-2 border-[#f5c31c] bg-[rgba(245,195,28,0.1)] px-4 py-3">
            <TriangleAlert className="mt-0.5 size-4.5 shrink-0 text-[#dba90e]" />
            <p className="text-sm font-semibold text-[#8a6d12]">
              この文字列は<strong>パスワードそのもの</strong>です。git に commit
              したり共有したりしないでください。漏れたら rotate
              で失効できます。<strong>内部入口</strong>なので、注入したサービスのコンテナからのみ接続できます。
            </p>
          </div>

          {cache && (
            <p className="text-xs font-medium text-muted-foreground">
              キー前缀(<code className="font-mono">REDIS_KEY_PREFIX</code>):
              <code className="font-mono font-semibold text-foreground">{cache.namespace}:</code>
              {" — "}注入時に <code className="font-mono">REDIS_URL</code> と一緒に渡されます。
            </p>
          )}

          {url ? (
            <div className="flex flex-col gap-2">
              <CodeBlock code={url} language="bash" showCopy />
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
        </section>

        <Divider type="line-brown" />

        {/* ===== 危険ゾーン ===== */}
        <section className="flex flex-col gap-3">
          <h2 className="text-lg font-bold text-[#c94444]">削除</h2>
          <p className="text-sm font-medium text-muted-foreground">
            削除するとゴミ箱に入ります(3 日間は復元可能。データはベストエフォートで残ります)。
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
            このキャッシュを削除
          </Button>
        </section>
      </div>

      {/* リネーム */}
      <Modal
        open={renameOpen}
        title="キャッシュ名を変更"
        typewriter={false}
        width={460}
        onClose={() => setRenameOpen(false)}
        footer={
          <>
            <Button type="text" onClick={() => setRenameOpen(false)}>
              キャンセル
            </Button>
            <Button
              type="primary"
              loading={rename.isPending}
              disabled={!renameName.trim()}
              onClick={submitRename}
            >
              変更
            </Button>
          </>
        }
      >
        <form
          onSubmit={(e) => {
            e.preventDefault();
            submitRename();
          }}
          className="flex w-full flex-col gap-3"
        >
          <Input
            label="名前"
            value={renameName}
            autoFocus
            onChange={(e) => setRenameName(e.target.value)}
            description="表示名だけ変わります。接続文字列・namespace はそのままです。"
          />
          {rename.error && (
            <p className="text-sm font-semibold text-[#e05a5a]">{rename.error.message}</p>
          )}
        </form>
      </Modal>

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
        title="キャッシュを削除"
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
              disabled={confirmName !== cache?.display_name}
              onClick={() =>
                del.mutate(id, {
                  onSuccess: () => {
                    setDeleteOpen(false);
                    navigate("/caches");
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
            確認のため、キャッシュ名 <strong>{cache?.display_name}</strong> を入力してください。
          </p>
          <Input
            value={confirmName}
            autoFocus
            placeholder={cache?.display_name}
            onChange={(e) => setConfirmName(e.target.value)}
          />
          {del.error && <p className="text-sm font-semibold text-[#e05a5a]">{del.error.message}</p>}
        </div>
      </Modal>
    </PageContainer>
  );
}
