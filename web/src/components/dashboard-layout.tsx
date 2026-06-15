import { useEffect } from "react";
import {
  BarChart3,
  ChevronRight,
  Gauge,
  KeyRound,
  LogOut,
  type LucideIcon,
  Menu,
  ScrollText,
  ShieldCheck,
  Users,
  X,
} from "lucide-react";
import { Link, NavLink, Navigate, Outlet, useLocation } from "react-router";

import { Button } from "@/components/ui/button";
import { useLogout, useMeQuery } from "@/lib/auth";
import { RESOURCES } from "@/lib/resources";
import { useUiStore } from "@/lib/store/ui";
import { cn } from "@/lib/utils";

// 管理画面の外殻:左サイドメニュー + 内容領域(<Outlet>)。ログイン守衛もここ。
// 状態は props で配らない:利用者(me)もログアウトも、必要な子が自分のフックで
// 直接読む(useMeQuery / useLogout)。Query が重複排除するので追加リクエストは出ない。
// 背景壁紙は body::before が全画面に敷くので、各パネルはその上に浮くクリーム面。
// 画面幅:md 以上は常設サイドバー、md 未満は上部バー + ドロワー(zustand 管理)。

// 読み込み中の全画面表示(ミントのリング)。
function FullPageLoading() {
  return (
    <div className="flex min-h-dvh items-center justify-center p-8">
      <div className="flex flex-col items-center gap-3">
        <div className="size-10 animate-spin rounded-full border-4 border-[#d4c9b4] border-t-primary" />
        <p className="text-sm font-bold text-muted-foreground">読み込み中…</p>
      </div>
    </div>
  );
}

// 管理セクションのナビ項目。リソースナビと同じ薬形 + active の葉っぱ揺れ。
// 複数項目(管制面 / ランキング / 監査ログ / 設定 / IP)で同じマークアップを繰り返すので束ねる。
function AdminNavLink({
  to,
  end,
  icon: Icon,
  label,
  onNavigate,
}: {
  to: string;
  end?: boolean;
  icon: LucideIcon;
  label: string;
  onNavigate?: () => void;
}) {
  return (
    <NavLink
      to={to}
      end={end}
      onClick={onNavigate}
      className={({ isActive }) =>
        cn(
          "relative flex items-center gap-3 rounded-2xl px-3.5 py-2.5 text-sm font-semibold text-foreground outline-none transition-all duration-250 ease-in-out focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-2",
          isActive
            ? "bg-[#0CC0B5] text-[#FFF9E3] shadow-[0_3px_0_0_rgba(61,52,40,0.08)]"
            : "hover:bg-[rgba(25,200,185,0.1)] hover:text-[#11a89b]",
        )
      }
    >
      {({ isActive }) => (
        <>
          <Icon className="size-5 shrink-0" />
          <span className="min-w-0 flex-1 truncate">{label}</span>
          {isActive && (
            <img
              src="/icons/icon-leaf.png"
              alt=""
              aria-hidden
              className="absolute -top-1 right-1 size-4.5 animate-[animal-leaf-wiggle_2s_ease-in-out_infinite]"
            />
          )}
        </>
      )}
    </NavLink>
  );
}

// サイドバーの中身(ブランド + ナビ + 利用者)。常設サイドバーとドロワーで共用する。
// onNavigate はドロワーから渡され、項目クリックで閉じる(常設側は渡さない)。
function SidebarContent({ onNavigate }: { onNavigate?: () => void }) {
  const { data: me } = useMeQuery();
  const logout = useLogout();

  return (
    <div className="flex h-full flex-col">
      {/* ブランド(クリックで はじめに へ) */}
      <NavLink
        to="/"
        end
        onClick={onNavigate}
        className="flex items-center gap-2.5 px-6 py-5 outline-none focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-2"
      >
        <img src="/logo.png" alt="" className="h-9 w-auto shrink-0" />
        <span className="flex min-w-0 flex-col leading-tight">
          <span className="text-2xl font-extrabold tracking-tight text-foreground">つぼみ</span>
          <span className="truncate text-[11px] font-bold text-muted-foreground">
            社内 PaaS プラットフォーム
          </span>
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
              onClick={onNavigate}
              className={({ isActive }) =>
                cn(
                  // 既定:茶文字・透明地・丸み薬形。
                  "relative flex items-center gap-3 rounded-2xl px-3.5 py-2.5 text-sm font-semibold text-foreground outline-none transition-all duration-250 ease-in-out focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-2",
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
                  <span className="min-w-0 flex-1 truncate">{r.label}</span>
                  {/* active 項目右上の葉っぱ(原典 Tabs と同じ icon-leaf を流用)。 */}
                  {isActive && (
                    <img
                      src="/icons/icon-leaf.png"
                      alt=""
                      aria-hidden
                      className="absolute -top-1 right-1 size-4.5 animate-[animal-leaf-wiggle_2s_ease-in-out_infinite]"
                    />
                  )}
                </>
              )}
            </NavLink>
          );
        })}
      </nav>

      {/* 管理。表示制御はただの UX — バックエンドが 403 で守る。管制面 / ランキングは
          閲覧(owner または共有パスワード viewer。未解錠なら解錠フォームへ)、監査ログ /
          共有パスワード設定 / IP 許可リストは owner のみ(§7 / S5)。 */}
      {me && (
        <nav className="flex flex-col gap-1.5 px-3 pb-2" aria-label="管理">
          <span className="px-3.5 pt-2 pb-0.5 text-[11px] font-bold tracking-wide text-muted-foreground">
            管理
          </span>
          {/* end:/admin は総覧のみ active(/admin/ranking では非 active にする)。 */}
          <AdminNavLink to="/admin" end icon={Gauge} label="管制面" onNavigate={onNavigate} />
          <AdminNavLink
            to="/admin/ranking"
            icon={BarChart3}
            label="使用量ランキング"
            onNavigate={onNavigate}
          />
          {me.role === "owner" && (
            <>
              <AdminNavLink
                to="/admin/audit"
                icon={ScrollText}
                label="監査ログ"
                onNavigate={onNavigate}
              />
              <AdminNavLink
                to="/admin/owners"
                icon={Users}
                label="Owner 管理"
                onNavigate={onNavigate}
              />
              <AdminNavLink
                to="/admin/settings"
                icon={KeyRound}
                label="共有パスワード設定"
                onNavigate={onNavigate}
              />
              <AdminNavLink
                to="/ip-allowlist"
                icon={ShieldCheck}
                label="IP 許可リスト"
                onNavigate={onNavigate}
              />
            </>
          )}
        </nav>
      )}

      {/* 利用者チップ + ログアウト */}
      {me && (
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
            <div className="flex min-w-0 flex-col">
              <span className="truncate text-sm font-bold text-foreground">
                {me.name ?? me.email}
              </span>
              <span className="truncate text-xs text-muted-foreground">
                {me.email} · {me.role}
              </span>
            </div>
          </div>
          <Button
            type="text"
            size="small"
            block
            loading={logout.isPending}
            icon={<LogOut className="size-4" />}
            onClick={() => logout.mutate()}
            className="justify-start"
          >
            ログアウト
          </Button>
        </div>
      )}
    </div>
  );
}

// md 未満のオーバーレイ・ドロワー。開閉は zustand。Esc とマスククリックで閉じる。
function MobileNav() {
  const navOpen = useUiStore((s) => s.navOpen);
  const closeNav = useUiStore((s) => s.closeNav);

  useEffect(() => {
    if (!navOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeNav();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [navOpen, closeNav]);

  if (!navOpen) return null;

  return (
    // Modal と同じマスク作法:外側 div がマスク(クリックで閉じる)、内側がパネル。
    <div
      className="fixed inset-0 z-1000 flex bg-[rgba(0,0,0,0.35)] animate-[animal-fade-in_0.2s_ease] md:hidden"
      onClick={closeNav}
    >
      {/* パネル(クリックは伝播させない) */}
      <div
        className="relative flex w-72 max-w-[80%] flex-col border-r-2 border-[#e8e2d6] bg-card shadow-[4px_0_16px_rgba(61,52,40,0.12)]"
        onClick={(e) => e.stopPropagation()}
      >
        <Button
          type="text"
          aria-label="メニューを閉じる"
          icon={<X className="size-5" />}
          onClick={closeNav}
          className="absolute top-4 right-3 z-10 size-9 rounded-full px-0"
        />
        <SidebarContent onNavigate={closeNav} />
      </div>
    </div>
  );
}

// 現在地ラベルを RESOURCES / index から導く(モバイルのパンくず用)。index は null。
function useCurrentLabel(): string | null {
  const { pathname } = useLocation();
  if (pathname === "/") return null;
  const match = RESOURCES.find((r) => pathname === r.path || pathname.startsWith(`${r.path}/`));
  return match?.label ?? null;
}

// md 未満の上部バー(ハンバーガー + パンくず)。現在地を示し、メニューはドロワーで開く。
function MobileTopBar() {
  const openNav = useUiStore((s) => s.openNav);
  const current = useCurrentLabel();
  return (
    <header className="sticky top-0 z-30 flex items-center gap-2 border-b-2 border-[#e8e2d6] bg-card px-4 py-3 md:hidden">
      <Button
        type="text"
        aria-label="メニューを開く"
        icon={<Menu className="size-5" />}
        onClick={openNav}
        className="size-9 shrink-0 rounded-full px-0"
      />
      {/* パンくず:ブランド(→ はじめに)+ 現在地 */}
      <nav aria-label="パンくず" className="flex min-w-0 items-center gap-1.5">
        <Link to="/" className="flex shrink-0 items-center gap-2 outline-none">
          <img src="/logo.png" alt="" className="h-7 w-auto" />
          <span className="text-lg font-extrabold tracking-tight text-foreground">つぼみ</span>
        </Link>
        {current && (
          <>
            <ChevronRight className="size-4 shrink-0 text-muted-foreground" aria-hidden />
            <span
              className="min-w-0 truncate text-sm font-bold text-foreground"
              aria-current="page"
            >
              {current}
            </span>
          </>
        )}
      </nav>
    </header>
  );
}

export function DashboardLayout() {
  const { data: me, isPending } = useMeQuery();

  if (isPending) return <FullPageLoading />;
  // 未ログインはログイン画面へ。replace で戻る矢印に守衛ループを残さない。
  if (!me) return <Navigate to="/login" replace />;

  return (
    <div className="flex min-h-dvh">
      {/* 常設サイドバー(md 以上) */}
      <aside className="sticky top-0 hidden h-dvh w-64 shrink-0 border-r-2 border-[#e8e2d6] bg-card md:block">
        <SidebarContent />
      </aside>

      {/* ドロワー(md 未満) */}
      <MobileNav />

      <div className="flex min-w-0 flex-1 flex-col">
        <MobileTopBar />
        {/* 横幅・パディングは各ページが <PageContainer> で宣言する。 */}
        <main className="min-w-0 flex-1">
          <Outlet />
        </main>
      </div>
    </div>
  );
}
