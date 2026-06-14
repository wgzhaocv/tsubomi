import { useState } from "react";
import { ArrowLeft, LayoutDashboard, SquareTerminal, Table2 } from "lucide-react";
import { Link, NavLink, Outlet, useParams } from "react-router";

import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { Title } from "@/components/ui/title";
import { useDatabases, useRenameDatabase } from "@/lib/databases";
import { cn } from "@/lib/utils";

// データベース詳細の外殻:戻りリンク + 見出し(+ リネーム)+ サブナビ(概要 / SQL /
// テーブル)。3 ページ(Overview / Editor / Tables)はこの <Outlet> に差さる。横幅は
// PageContainer の wide(表データの横スクロール用)。

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

  const rename = useRenameDatabase(id);
  const [renameOpen, setRenameOpen] = useState(false);
  const [renameName, setRenameName] = useState("");

  const submitRename = () => {
    const trimmed = renameName.trim();
    if (!trimmed || rename.isPending) return; // 二重送信を防ぐ
    rename.mutate(trimmed, { onSuccess: () => setRenameOpen(false) });
  };

  return (
    <PageContainer width="wide">
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
            {db ? (
              // 名前そのものをクリックでリネーム Modal。鉛筆などの装飾は出さず、hover は
              // リボン色をひと段明るくするだけ(控えめな手掛かり)。a11y は本物の
              // <button>(キーボード可・focus-visible 描画)で担保。
              <button
                type="button"
                aria-label="データベース名を変更"
                title="クリックして名前を変更"
                onClick={() => {
                  setRenameName(db.display_name);
                  setRenameOpen(true);
                }}
                className="group w-fit cursor-pointer rounded-2xl outline-none focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-4"
              >
                <Title
                  size="large"
                  color="app-blue"
                  className="group-hover:[--rb:#6a80e2] group-hover:[--rf:#9fb1f5]"
                >
                  {db.display_name}
                </Title>
              </button>
            ) : (
              <Title size="large" color="app-blue">
                {id}
              </Title>
            )}
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

      {/* リネーム(表示名のみ。接続文字列・dbname は不変)。 */}
      <Modal
        open={renameOpen}
        title="データベース名を変更"
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
            description="表示名だけ変わります。接続文字列・データベース名はそのままです。"
          />
          {rename.error && (
            <p className="text-sm font-semibold text-[#e05a5a]">{rename.error.message}</p>
          )}
        </form>
      </Modal>
    </PageContainer>
  );
}
