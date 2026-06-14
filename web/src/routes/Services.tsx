import { useState } from "react";
import { Plus, Server } from "lucide-react";
import { useNavigate } from "react-router";

import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { PhaseBadge } from "@/components/phase-badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { CodeBlock } from "@/components/ui/codeblock";
import { Divider } from "@/components/ui/divider";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { Title } from "@/components/ui/title";
import {
  type CreateServiceResult,
  type Service,
  useCreateService,
  useServices,
} from "@/lib/services";

// サービス一覧 + 作成導線。サービスは GitHub repo と 1:1 のデプロイ単位。
// 平台は GitHub に触れないので、作成後は「次の一手」(setup_commands + workflow)を
// 表示する(CLI の json「方案だけ返す」と同じ思想)。カードクリックで詳細ページへ。

// モーダルの状態(作成フォーム / 作成後の連携手順 / 閉)を 1 つの型で表す。
type ModalState = { kind: "create" } | { kind: "setup"; result: CreateServiceResult } | null;

export default function Services() {
  const navigate = useNavigate();
  const { data: services, isPending, error } = useServices();
  const create = useCreateService();

  const [modal, setModal] = useState<ModalState>(null);
  const [name, setName] = useState("");

  const submit = () => {
    const trimmed = name.trim();
    if (!trimmed || create.isPending) return; // 二重送信を防ぐ
    create.mutate(trimmed, {
      onSuccess: (svc) => {
        setName("");
        setModal({ kind: "setup", result: svc });
      },
    });
  };

  const setup = modal?.kind === "setup" ? modal.result : null;

  return (
    <PageContainer>
      <div className="flex flex-col gap-7">
        <PageMeta title="サービス" />

        <header className="flex flex-wrap items-center justify-between gap-4">
          <Title size="large" color="app-teal">
            サービス
          </Title>
          {services && services.length > 0 && (
            <Button
              type="default"
              icon={<Plus className="size-4" />}
              onClick={() => setModal({ kind: "create" })}
            >
              サービスを作成
            </Button>
          )}
        </header>

        <Divider type="line-brown" />

        {error && (
          <p className="text-sm font-semibold text-[#e05a5a]">
            読み込みに失敗しました:{error.message}
          </p>
        )}

        {!isPending && services && services.length === 0 && (
          <Card type="dashed">
            <CardContent className="flex flex-col items-center gap-4 px-6 py-12 text-center">
              <div className="grid size-16 place-items-center rounded-full bg-accent text-accent-foreground">
                <Server className="size-8" />
              </div>
              <div className="flex flex-col gap-1.5">
                <p className="text-lg font-bold text-foreground">まだサービスがありません</p>
                <p className="max-w-md text-sm font-medium text-muted-foreground">
                  GitHub リポジトリと 1 対 1 で結びつくデプロイ単位です。作成すると連携手順 (gh /
                  workflow)が表示され、git push で自動デプロイされます。
                </p>
              </div>
              <Button
                type="primary"
                icon={<Plus className="size-4" />}
                onClick={() => setModal({ kind: "create" })}
              >
                サービスを作成
              </Button>
            </CardContent>
          </Card>
        )}

        {services && services.length > 0 && (
          <ul className="flex flex-col gap-3">
            {services.map((svc: Service) => (
              <li key={svc.id}>
                <Card
                  interactive
                  onClick={() => navigate(`/services/${svc.id}`)}
                  className="flex-row items-center justify-between gap-4 py-4"
                >
                  <CardContent className="flex min-w-0 items-center gap-3.5">
                    <div className="grid size-11 shrink-0 place-items-center rounded-2xl bg-accent text-accent-foreground">
                      <Server className="size-5.5" />
                    </div>
                    <div className="flex min-w-0 flex-col">
                      <span className="truncate text-base font-bold text-foreground">
                        {svc.display_name}
                      </span>
                      <span className="truncate text-xs font-medium text-muted-foreground">
                        {svc.subdomain} · service{svc.anon_seq} · 作成{" "}
                        {new Date(svc.created_at).toLocaleDateString("ja-JP")}
                      </span>
                    </div>
                  </CardContent>
                  <PhaseBadge phase={svc.phase} />
                </Card>
              </li>
            ))}
          </ul>
        )}

        {/* 作成モーダル(名前を 1 つ)。 */}
        <Modal
          open={modal?.kind === "create"}
          title="サービスを作成"
          typewriter={false}
          onClose={() => setModal(null)}
          width={460}
          footer={
            <>
              <Button type="text" onClick={() => setModal(null)}>
                キャンセル
              </Button>
              <Button type="primary" loading={create.isPending} onClick={submit}>
                作成
              </Button>
            </>
          }
        >
          <form
            onSubmit={(e) => {
              e.preventDefault();
              submit();
            }}
            className="flex w-full flex-col gap-3"
          >
            <Input
              label="名前"
              placeholder="例:myapp"
              value={name}
              autoFocus
              onChange={(e) => setName(e.target.value)}
              description="表示名です。GitHub リポジトリ名には自動生成の subdomain を使います。"
            />
            {create.error && (
              <p className="text-sm font-semibold text-[#e05a5a]">{create.error.message}</p>
            )}
          </form>
        </Modal>

        {/* 作成成功後の連携手順(一度だけ。deploy_key など秘密を含む)。 */}
        <Modal
          open={setup != null}
          title="サービスを作成しました — 連携手順"
          typewriter={false}
          onClose={() => setModal(null)}
          width={760}
          footer={
            <Button type="primary" onClick={() => setModal(null)}>
              閉じる
            </Button>
          }
        >
          {setup && (
            <div className="flex w-full flex-col gap-4">
              <div className="rounded-xl border border-[#e05a5a]/40 bg-[#e05a5a]/10 px-4 py-3">
                <p className="text-sm font-bold text-[#e05a5a]">
                  ⚠ deploy key と registry パスワードはここでしか表示されません
                </p>
                <p className="mt-1 text-xs font-medium text-muted-foreground">
                  共有・git へのコミットはしないこと。紛失したら新しいサービスを作り直してください。
                </p>
              </div>

              <p className="text-sm font-medium text-foreground">
                リポジトリ直下で以下を実行 → <code className="font-mono">git push</code>{" "}
                で自動デプロイ。
                <span className="text-muted-foreground">
                  {" "}
                  (gh が無ければ各値を手動で GitHub Secrets / Variables に登録)
                </span>
              </p>

              <CodeBlock
                title="GitHub 連携(gh)"
                language="sh"
                code={setup.setup_commands.join("\n")}
                showCopy
              />

              <CodeBlock
                title=".github/workflows/tsubomi-deploy.yml"
                language="yaml"
                code={setup.workflow_yaml}
                showCopy
              />
            </div>
          )}
        </Modal>
      </div>
    </PageContainer>
  );
}
