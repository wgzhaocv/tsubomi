import {
  ArrowRight,
  Boxes,
  ChevronDown,
  Database,
  GitBranch,
  Globe,
  HardDrive,
  type LucideIcon,
  Package,
  Server,
  Syringe,
  Terminal,
  Users,
  Zap,
} from "lucide-react";
import { type ReactNode } from "react";
import { Link } from "react-router";

import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Divider } from "@/components/ui/divider";
import { Title } from "@/components/ui/title";
import { Typewriter } from "@/components/ui/typewriter";

// プロジェクト紹介の公開ページ(ログイン不要 = 守衛の外)。同僚にアーキテクチャを「図」で
// 見せるのが主目的なので、本文は最小限にして 1 枚の構成図を主役にする。内容は実装に即した事実。

// 図の 1 ボックス(アイコンチップ + ラベル + 小注記)。imgSrc を渡すと lucide の代わりに
// 画像(ロゴ)をチップに出す。
function Box({
  icon: Icon,
  imgSrc,
  label,
  sub,
  accent,
  highlight,
}: {
  icon?: LucideIcon;
  imgSrc?: string;
  label: string;
  sub?: string;
  accent: string;
  highlight?: boolean;
}) {
  return (
    <div
      className={
        "flex flex-1 items-center gap-2.5 rounded-2xl border-2 bg-card px-3 py-2.5 shadow-[0_3px_0_0_#d8ccae] " +
        (highlight ? "border-[#0CC0B5]" : "border-[#c4b89e]")
      }
    >
      {imgSrc ? (
        <span className="grid size-9 shrink-0 place-items-center rounded-xl border-2 border-[#e3dcc9] bg-[#fffdf5]">
          <img src={imgSrc} alt="" className="size-6 object-contain" />
        </span>
      ) : (
        <span
          className="grid size-9 shrink-0 place-items-center rounded-xl text-[#FFF9E3]"
          style={{ backgroundColor: accent }}
        >
          {Icon && <Icon className="size-4.5" strokeWidth={2.5} />}
        </span>
      )}
      <div className="min-w-0">
        <p className="text-[13px] leading-tight font-bold text-foreground">{label}</p>
        {sub && <p className="text-[11px] leading-tight font-medium text-foreground/55">{sub}</p>}
      </div>
    </div>
  );
}

// 1 つの層(帯)。左上に番号 + 層名のタグ。
function Layer({ tag, children }: { tag: string; children: ReactNode }) {
  return (
    <div className="relative rounded-2xl border-2 border-dashed border-[#d8ccae] bg-[#fffdf5] px-3 pt-7 pb-3">
      <span className="absolute -top-2.5 left-3 rounded-full bg-[#0CC0B5] px-2.5 py-0.5 text-[11px] font-black tracking-wide text-[#FFF9E3]">
        {tag}
      </span>
      <div className="flex flex-col gap-2.5 sm:flex-row">{children}</div>
    </div>
  );
}

// 層と層をつなぐ下向きの矢印(中央のシェブロン + ラベル)。
function Down({ label }: { label?: string }) {
  return (
    <div className="flex flex-col items-center gap-0.5 py-1">
      <span className="h-3 w-0.5 rounded-full bg-[#c4b89e]" />
      <ChevronDown className="-my-1 size-4 text-[#b3a589]" strokeWidth={3} />
      {label && (
        <span className="rounded-full bg-card px-2 py-0.5 text-[11px] font-semibold text-foreground/55">
          {label}
        </span>
      )}
    </div>
  );
}

export default function About() {
  return (
    <main className="flex min-h-dvh flex-col items-center px-5 py-10 text-foreground">
      <PageMeta
        title="このプラットフォームについて"
        description="社内向け PaaS「蕾(tsubomi)」のアーキテクチャ図"
      />

      <div className="flex w-full max-w-3xl flex-col gap-7">
        {/* ヒーロー(最小限) */}
        <header className="flex flex-col items-center gap-2.5 text-center">
          <img src="/logo.png" alt="" className="h-11 w-auto" />
          <Title size="large" color="app-teal">
            アーキテクチャ
          </Title>
          <h1 className="text-2xl font-extrabold tracking-tight">蕾 — tsubomi</h1>
          <p className="text-sm font-semibold text-foreground/75">
            <Typewriter>社内向け PaaS ──「基礎版 Vercel + Neon」をセルフホスト。</Typewriter>
          </p>
        </header>

        <Divider type="dashed-teal" />

        {/* 構成図(主役) */}
        <div className="flex flex-col">
          {/* ① 使う人・つくる人 */}
          <Layer tag="① 使う人・つくる人">
            <Box icon={Users} label="社内ユーザ" sub="ブラウザで開く" accent="#5BA8E0" />
            <Box icon={Terminal} label="Claude Code + tbm" sub="AI に頼んで作る" accent="#0CC0B5" />
            <Box icon={GitBranch} label="GitHub" sub="git push / Actions" accent="#9B7BD4" />
          </Layer>

          <Down label="ブラウザ ・ デプロイ ・ push" />

          {/* ② エッジ。公開入口 / TLS 終端は配備で差し替え可(現在は CF Tunnel、直VPS は Traefik+LE)。 */}
          <Layer tag="② エッジ(公開入口)">
            <Box
              icon={Globe}
              label="TLS 終端・公開入口"
              sub="Cloudflare Tunnel / 逆代理 / Traefik+LE ── 配備で差し替え可"
              accent="#E89A4B"
            />
            <Box
              icon={Boxes}
              label="Traefik"
              sub="ルーティング・会社 IP 許可リスト"
              accent="#9B7BD4"
            />
          </Layer>

          <Down label="*.<ドメイン> ・ 管理 API" />

          {/* ③ 中核 */}
          <Layer tag="③ 中核(ホスト上で直接稼働)">
            <Box
              imgSrc="/logo.png"
              label="tsubomi サーバ(Rust)"
              sub="docker.sock を握る頭脳・現実をあるべき状態に合わせ続ける"
              accent="#0CC0B5"
              highlight
            />
            {/* 注入の横矢印(サーバ → app)。狭い画面では下向きになる。 */}
            <div className="flex items-center justify-center gap-1 px-1 text-[#11a89b]">
              <Syringe className="size-4 rotate-90 sm:rotate-0" strokeWidth={2.5} />
              <span className="text-[11px] font-black">注入</span>
              <ArrowRight className="hidden size-4 sm:block" strokeWidth={3} />
            </div>
            <Box
              icon={Package}
              label="ユーザの app コンテナ"
              sub="エッジ網・メモリ上限・無停止で入れ替え"
              accent="#7CC47C"
            />
          </Layer>

          <Down label="コンテナを起こす・揃える(docker.sock)" />

          {/* ④ 管理下のリソース */}
          <Layer tag="④ 管理下のリソース">
            <Box
              icon={Database}
              label="PostgreSQL"
              sub="プラットフォーム用(あるべき状態)+ ユーザの DB(pgbouncer 経由)"
              accent="#5BA8E0"
            />
            <Box icon={Zap} label="Valkey" sub="キャッシュ・ACL 隔離" accent="#E8B84B" />
            <Box icon={Package} label="Registry" sub="app イメージ" accent="#E89A4B" />
            <Box icon={HardDrive} label="Volumes" sub="永続ディスク" accent="#7CC47C" />
          </Layer>
        </div>

        {/* 図の読み方(1 行ずつの注記。本文ではなく凡例) */}
        <div className="flex flex-col gap-2 rounded-2xl border-2 border-[#e3dcc9] bg-card/70 px-4 py-3">
          <p className="flex items-center gap-2 text-xs font-medium text-foreground/75">
            <Syringe className="size-4 shrink-0 text-[#11a89b]" strokeWidth={2.5} />
            <span>
              <strong className="font-bold text-foreground">注入</strong> ＝ リソース同士の結びつき
              だけを保存し、実際の値は
              <strong className="font-bold text-foreground">起動の瞬間</strong>
              に解決(動詞はこれ 1 つ)。
            </span>
          </p>
          <p className="flex items-center gap-2 text-xs font-medium text-foreground/75">
            <Server className="size-4 shrink-0 text-[#11a89b]" strokeWidth={2.5} />
            <span>
              中核は <strong className="font-bold text-foreground">Rust</strong> 製で 8MB バイナリ・
              待機 ~5MB。サーバ 1 台で運用・ARM64 / x86_64 両対応。隔離は「仕組み」で守る。
            </span>
          </p>
        </div>

        {/* 導線 */}
        <div className="flex flex-wrap items-center justify-center gap-3">
          <Button asChild type="primary" size="middle">
            <Link to="/">はじめる 🌷</Link>
          </Button>
          <Button asChild type="default" size="middle">
            <Link to="/cli">tbm CLI を入れる</Link>
          </Button>
        </div>
      </div>
    </main>
  );
}
