import { useEffect, useState } from "react";
import {
  ArrowLeft,
  History,
  LayoutDashboard,
  ScrollText,
  SlidersHorizontal,
  SquareTerminal,
} from "lucide-react";
import { Link, NavLink, Outlet, useParams } from "react-router";

import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { PhaseBadge } from "@/components/phase-badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { Title } from "@/components/ui/title";
import { useRenameService, useService } from "@/lib/services";
import { cn } from "@/lib/utils";

// サービス詳細の外殻:戻りリンク + 見出し(phase バッジ + リネーム)+ サブナビ(概要 /
// デプロイ / 環境変数 / ログ / ターミナル)。各ページはこの <Outlet> に差さる。
// DatabaseLayout / VolumeLayout と同じ構造(見出しクリックでリネーム)。
// 注入は環境変数タブに統合済み(容器が受け取る変数の全体像を 1 画面で。注入は「注入」バッジで特別表示)。

const NAV = [
  { to: "", end: true, label: "概要", icon: LayoutDashboard },
  { to: "deploys", end: false, label: "デプロイ", icon: History },
  { to: "env", end: false, label: "環境変数", icon: SlidersHorizontal },
  { to: "logs", end: false, label: "ログ", icon: ScrollText },
  { to: "terminal", end: false, label: "ターミナル", icon: SquareTerminal },
] as const;

export default function ServiceLayout() {
  const { id = "" } = useParams();
  const { data: svc } = useService(id);

  const rename = useRenameService(id);
  const [renameOpen, setRenameOpen] = useState(false);
  const [renameName, setRenameName] = useState("");
  // 同じ route(/services/:id)間の遷移では Layout が再マウントされないため、開いたままの
  // modal が**別サービス**に送信される事故を防ぐ(codex 監査 2026-08-13)。
  useEffect(() => setRenameOpen(false), [id]);

  const submitRename = () => {
    const trimmed = renameName.trim();
    if (!trimmed || rename.isPending) return; // 二重送信を防ぐ
    rename.mutate(trimmed, { onSuccess: () => setRenameOpen(false) });
  };

  return (
    <PageContainer>
      <div className="flex flex-col gap-6">
        <PageMeta title={svc ? svc.display_name : "サービス"} />

        <div className="flex flex-col gap-3">
          <Link
            to="/services"
            className="inline-flex w-fit items-center gap-1.5 text-sm font-semibold text-muted-foreground outline-none hover:text-[#11a89b] focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-2"
          >
            <ArrowLeft className="size-4" />
            サービス一覧へ
          </Link>
          <header className="flex flex-wrap items-center justify-between gap-4">
            <div className="flex flex-wrap items-center gap-3">
              {svc ? (
                <button
                  type="button"
                  aria-label="サービス名を変更"
                  title="クリックして名前を変更"
                  onClick={() => {
                    setRenameName(svc.display_name);
                    setRenameOpen(true);
                  }}
                  className="group w-fit cursor-pointer rounded-2xl outline-none focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-4"
                >
                  <Title
                    size="large"
                    color="app-teal"
                    className="group-hover:[--rb:#0aa79d] group-hover:[--rf:#2adfd2]"
                  >
                    {svc.display_name}
                  </Title>
                </button>
              ) : (
                <Title size="large" color="app-teal">
                  {id}
                </Title>
              )}
              {svc && <PhaseBadge phase={svc.phase} />}
            </div>
            {svc && (
              <span className="rounded-full bg-accent px-3 py-1 text-xs font-bold text-accent-foreground">
                service{svc.anon_seq}
              </span>
            )}
          </header>
        </div>

        <nav
          className="flex flex-wrap gap-1.5 border-b-2 border-[#e8e2d6] pb-3"
          aria-label="サービスのページ"
        >
          {NAV.map((n) => {
            const Icon = n.icon;
            return (
              <NavLink
                key={n.to}
                to={n.to}
                end={n.end}
                className={({ isActive }) =>
                  cn(
                    "flex items-center gap-2 rounded-2xl px-3.5 py-2 text-sm font-semibold outline-none transition-all duration-250 ease-in-out focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-2",
                    isActive
                      ? "bg-[#0CC0B5] text-[#FFF9E3] shadow-[0_3px_0_0_rgba(61,52,40,0.08)]"
                      : "text-foreground hover:bg-[rgba(25,200,185,0.1)] hover:text-[#11a89b]",
                  )
                }
              >
                <Icon className="size-4.5 shrink-0" />
                {n.label}
              </NavLink>
            );
          })}
        </nav>

        {/* Outlet 配下を id で強制再マウント:同じ route(/:id)間の遷移では子が再マウント
            されず、開いたままの modal・取得済みの秘密・編集途中のフォームが**別リソース**に
            持ち越される(codex 審査 2026-08-13)。key で子の state を丸ごと畳む。 */}
        <Outlet key={id} />
      </div>

      {/* リネーム(表示名のみ。subdomain = 公開 URL / GitHub repo は不変)。 */}
      <Modal
        open={renameOpen}
        title="サービス名を変更"
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
            description="表示名だけ変わります。公開 URL(subdomain)と GitHub repo はそのままです。"
          />
          {rename.error && (
            <p className="text-sm font-semibold text-[#e05a5a]">{rename.error.message}</p>
          )}
        </form>
      </Modal>
    </PageContainer>
  );
}
