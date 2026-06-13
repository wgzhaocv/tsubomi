import { LogOut } from "lucide-react";
import { NavLink, Navigate, Outlet, useOutletContext } from "react-router";

import { Button } from "@/components/ui/button";
import { logout, useMe, type Me } from "@/lib/auth";
import { RESOURCES } from "@/lib/resources";
import { cn } from "@/lib/utils";

// 管理画面の外殻:左サイドメニュー + 内容領域(<Outlet>)。
// ログイン守衛もここで行う(未ログインなら /login へ)。背景の壁紙は body::before が
// 全画面に敷くので、サイドバー・内容はその上に浮くクリーム面パネルとして置く。
// 画面幅:md 未満はアイコンのみの細い縦バー(w-20)、md 以上はラベル付き(w-64)。

// 子ルートへ渡す文脈(今はログインユーザのみ)。Welcome で名前表示に使う。
export type DashboardContext = { me: Me };

export function useDashboard() {
  return useOutletContext<DashboardContext>();
}

// 読み込み中の全画面表示(ミントのリング)。背景壁紙の上に中央寄せ。
function FullPageLoading() {
  return (
    <div className="flex min-h-dvh items-center justify-center p-8">
      <div className="flex flex-col items-center gap-3">
        <div className="size-10 animate-spin rounded-full border-4 border-[#d4c9b4] border-t-[#19c8b9]" />
        <p className="text-sm font-bold text-muted-foreground">読み込み中…</p>
      </div>
    </div>
  );
}

function Sidebar({ me, onLogout }: { me: Me; onLogout: () => void }) {
  return (
    // sticky で全高に貼り付ける。右に淡い 2px の区切り。クリーム面。
    <aside className="sticky top-0 flex h-dvh w-20 shrink-0 flex-col border-r-2 border-[#e8e2d6] bg-[rgb(247,243,223)] md:w-64">
      {/* ブランド(クリックで はじめに へ) */}
      <NavLink
        to="/"
        end
        className="flex items-center gap-2.5 px-4 py-5 outline-none focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-2 md:px-6"
      >
        <img src="/logo.png" alt="" className="h-9 w-auto shrink-0" />
        <span className="hidden text-2xl font-extrabold tracking-tight text-foreground md:inline">
          つぼみ
        </span>
      </NavLink>

      {/* リソースのナビ。RESOURCES 設定から生成(順序もそこで決まる)。 */}
      <nav className="flex flex-1 flex-col gap-1.5 overflow-y-auto px-3 py-2" aria-label="リソース">
        {RESOURCES.map((r) => {
          const Icon = r.icon;
          return (
            <NavLink
              key={r.path}
              to={r.path}
              aria-label={r.label}
              title={r.label}
              className={({ isActive }) =>
                cn(
                  // 既定:茶文字・透明地・丸み。md 未満は中央寄せ(アイコンのみ)。
                  "relative flex items-center gap-3 rounded-2xl px-3 py-2.5 text-sm font-semibold text-[#794f27] outline-none transition-all duration-250 ease-in-out focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-2 max-md:justify-center",
                  isActive
                    ? // active:ミント面・クリーム文字・下方向の立体影(Tabs の active と同じ語彙)
                      "bg-[#0CC0B5] text-[#FFF9E3] shadow-[0_3px_0_0_rgba(61,52,40,0.08)]"
                    : "hover:bg-[rgba(25,200,185,0.1)] hover:text-[#11a89b]",
                )
              }
            >
              {({ isActive }) => (
                <>
                  <Icon className="size-5 shrink-0" />
                  <span className="hidden md:inline">{r.label}</span>
                  {/* active タブ右上の葉っぱ(原典 Tabs と同じ icon-leaf を流用)。 */}
                  {isActive && (
                    <img
                      src="/icons/icon-leaf.png"
                      alt=""
                      aria-hidden
                      className="absolute -top-1 -right-[5px] size-[18px] animate-[animal-leaf-wiggle_2s_ease-in-out_infinite]"
                    />
                  )}
                </>
              )}
            </NavLink>
          );
        })}
      </nav>

      {/* 利用者チップ + ログアウト */}
      <div className="mt-auto flex flex-col gap-2 border-t-2 border-[#e8e2d6] p-3">
        <div className="flex items-center gap-2.5 px-1">
          {me.avatar_url ? (
            <img
              src={me.avatar_url}
              alt=""
              className="size-9 shrink-0 rounded-full border-2 border-border"
            />
          ) : (
            <div className="grid size-9 shrink-0 place-items-center rounded-full border-2 border-border bg-accent text-sm font-bold text-accent-foreground">
              {(me.name ?? me.email).slice(0, 1).toUpperCase()}
            </div>
          )}
          <div className="hidden min-w-0 flex-col md:flex">
            <span className="truncate text-sm font-bold text-foreground">{me.name ?? me.email}</span>
            <span className="truncate text-xs text-muted-foreground">
              {me.email} · {me.role}
            </span>
          </div>
        </div>
        <Button
          type="text"
          size="small"
          block
          aria-label="ログアウト"
          icon={<LogOut className="size-4" />}
          onClick={onLogout}
          className="max-md:justify-center md:justify-start"
        >
          <span className="hidden md:inline">ログアウト</span>
        </Button>
      </div>
    </aside>
  );
}

export function DashboardLayout() {
  const { me, loading, setMe } = useMe();

  if (loading) return <FullPageLoading />;
  // 未ログインはログイン画面へ。replace で戻る矢印に守衛ループを残さない。
  if (!me) return <Navigate to="/login" replace />;

  const handleLogout = () => {
    void logout().then(() => setMe(null));
  };

  return (
    <div className="flex min-h-dvh">
      <Sidebar me={me} onLogout={handleLogout} />
      <main className="min-w-0 flex-1">
        <div className="mx-auto w-full max-w-5xl p-6 md:p-10">
          <Outlet context={{ me } satisfies DashboardContext} />
        </div>
      </main>
    </div>
  );
}
