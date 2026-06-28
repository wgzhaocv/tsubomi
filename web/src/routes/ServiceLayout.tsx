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
import { Title } from "@/components/ui/title";
import { useService } from "@/lib/services";
import { cn } from "@/lib/utils";

// サービス詳細の外殻:戻りリンク + 見出し(phase バッジ)+ サブナビ(概要 / デプロイ /
// 環境変数 / ログ / ターミナル)。各ページはこの <Outlet> に差さる。DatabaseLayout と同じ構造。
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
              <Title size="large" color="app-teal">
                {svc ? svc.display_name : id}
              </Title>
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

        <Outlet />
      </div>
    </PageContainer>
  );
}
