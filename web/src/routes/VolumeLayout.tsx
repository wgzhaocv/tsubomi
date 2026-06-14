import { useState } from "react";
import { ArrowLeft, FolderOpen, LayoutDashboard } from "lucide-react";
import { Link, NavLink, Outlet, useParams } from "react-router";

import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { Title } from "@/components/ui/title";
import { useRenameVolume, useVolumes } from "@/lib/volumes";
import { cn } from "@/lib/utils";

// ボリューム詳細の外殻:戻りリンク + 見出し(+ リネーム)+ サブナビ(概要 / ファイル)。
// 2 ページ(Overview / FileBrowser)はこの <Outlet> に差さる。横幅は wide
// (ファイル一覧テーブルの横スクロール用)。DatabaseLayout と同じ作法。

const NAV = [
  { to: "", end: true, label: "概要", icon: LayoutDashboard },
  { to: "files", end: false, label: "ファイル", icon: FolderOpen },
] as const;

export default function VolumeLayout() {
  const { id = "" } = useParams();
  const { data: volumes } = useVolumes();
  const vol = volumes?.find((v) => v.id === id);

  const rename = useRenameVolume(id);
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
        <PageMeta title={vol ? vol.display_name : "ボリューム"} />

        <div className="flex flex-col gap-3">
          <Link
            to="/volumes"
            className="inline-flex w-fit items-center gap-1.5 text-sm font-semibold text-muted-foreground outline-none hover:text-[#11a89b] focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-2"
          >
            <ArrowLeft className="size-4" />
            ボリューム一覧へ
          </Link>
          <header className="flex flex-wrap items-center justify-between gap-4">
            {vol ? (
              <button
                type="button"
                aria-label="ボリューム名を変更"
                title="クリックして名前を変更"
                onClick={() => {
                  setRenameName(vol.display_name);
                  setRenameOpen(true);
                }}
                className="group w-fit cursor-pointer rounded-2xl outline-none focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-4"
              >
                <Title
                  size="large"
                  color="app-yellow"
                  className="group-hover:[--rb:#d8a90e] group-hover:[--rf:#f5d24a]"
                >
                  {vol.display_name}
                </Title>
              </button>
            ) : (
              <Title size="large" color="app-yellow">
                {id}
              </Title>
            )}
            {vol && (
              <span className="rounded-full bg-accent px-3 py-1 text-xs font-bold text-accent-foreground">
                volume{vol.anon_seq}
              </span>
            )}
          </header>
        </div>

        <nav
          className="flex flex-wrap gap-1.5 border-b-2 border-[#e8e2d6] pb-3"
          aria-label="ボリュームのページ"
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

      {/* リネーム(表示名のみ。host_path・ファイルは不変)。 */}
      <Modal
        open={renameOpen}
        title="ボリューム名を変更"
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
            description="表示名だけ変わります。保存したファイルはそのままです。"
          />
          {rename.error && (
            <p className="text-sm font-semibold text-[#e05a5a]">{rename.error.message}</p>
          )}
        </form>
      </Modal>
    </PageContainer>
  );
}
