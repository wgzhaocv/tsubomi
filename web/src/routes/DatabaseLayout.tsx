import { ArrowLeft, LayoutDashboard, SquareTerminal, Table2 } from "lucide-react";
import { Link, NavLink, Outlet, useParams } from "react-router";

import { PageMeta } from "@/components/page-meta";
import { Title } from "@/components/ui/title";
import { useDatabases } from "@/lib/databases";
import { cn } from "@/lib/utils";

// データベース詳細の外殻:戻りリンク + 見出し + サブナビ(概要 / SQL / テーブル)。
// 3 ページ(Overview / Editor / Tables)はこの <Outlet> に差さる。横幅は
// DashboardLayout 側で /databases/:id を全幅にしている(表データの横スクロール用)。

// サブナビ 1 項目。NavLink の active 配色は左メニュー(dashboard-layout)と同じ語彙。
const NAV = [
  { to: "", end: true, label: "概要", icon: LayoutDashboard },
  { to: "editor", end: false, label: "SQL", icon: SquareTerminal },
  { to: "tables", end: false, label: "テーブル", icon: Table2 },
] as const;

export default function DatabaseLayout() {
  const { id = "" } = useParams();
  const { data: dbs } = useDatabases();
  const db = dbs?.find((d) => d.id === id);

  return (
    <div className="flex flex-col gap-6">
      <PageMeta title={db ? db.display_name : "データベース"} />

      <div className="flex flex-col gap-3">
        <Link
          to="/databases"
          className="inline-flex w-fit items-center gap-1.5 text-sm font-semibold text-muted-foreground outline-none hover:text-[#11a89b] focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-2"
        >
          <ArrowLeft className="size-4" />
          データベース一覧へ
        </Link>
        <header className="flex flex-wrap items-center justify-between gap-4">
          <Title size="large" color="app-blue">
            {db ? db.display_name : id}
          </Title>
          {db && (
            <span className="rounded-full bg-accent px-3 py-1 text-xs font-bold text-accent-foreground">
              database{db.anon_seq}
            </span>
          )}
        </header>
      </div>

      {/* サブナビ:薬形ピル。区切り線の上に乗せる(下に内容が続く)。 */}
      <nav
        className="flex flex-wrap gap-1.5 border-b-2 border-[#e8e2d6] pb-3"
        aria-label="データベースのページ"
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
  );
}
