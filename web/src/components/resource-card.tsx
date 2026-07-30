import type { ReactNode } from "react";

import { Card } from "@/components/ui/card";

// 一覧ページ(Services / Databases / Caches / Volumes)共通のリソースカード。
// Vercel のダッシュボードカードを参照した三層縦積み:ヘッダ(アイコン + 名前 + バッジ)/
// 本文(説明 1 行 = description + チップ列 = children)/ フッタ(匿名番号・日時)。
// 見た目のトークン(丸み・クリーム面・hover 浮上)は ui/card.tsx に任せ、ここはレイアウトだけ。
// 4 ページが「1 つのシステム」に見えるための不変量(グリッド寸法・行のスタイル・アイコン寸法)は
// 全部このファイルに置く — 利用側に同じ className を 4 回書かせない。

// 一覧のグリッド(カード幅の下限 24rem = 狭い画面では 100% に折れる)。
export function ResourceCardGrid({ children }: { children: ReactNode }) {
  return (
    <ul className="grid grid-cols-[repeat(auto-fill,minmax(min(24rem,100%),1fr))] gap-4">
      {children}
    </ul>
  );
}

// チップ(本文のメタ情報 1 粒)。カード内でしか使わないのでここに同居。
export function CardChip({ children }: { children: ReactNode }) {
  return (
    <span className="rounded-full bg-accent/60 px-2.5 py-0.5 text-xs font-semibold text-accent-foreground">
      {children}
    </span>
  );
}

export function ResourceCard({
  icon,
  title,
  badge,
  description,
  footer,
  onClick,
  children,
}: {
  /// ヘッダ左のアイコン(裸で渡す — 寸法はタイル側の `*:size-5.5` が与える)。
  icon: ReactNode;
  title: string;
  /// ヘッダ右端のバッジ(service の PhaseBadge 等)。無ければ省略。
  badge?: ReactNode;
  /// 本文 1 行目(Vercel のドメイン行に相当。URL・key 接頭辞など)。
  description?: ReactNode;
  /// 底辺のメタ行(匿名番号 · 日時)。
  footer: ReactNode;
  onClick: () => void;
  /// 本文のチップ列などの追加行。
  children?: ReactNode;
}) {
  return (
    <Card interactive onClick={onClick} className="h-full gap-3 px-6 py-5">
      <div className="flex items-center gap-3.5">
        <div className="grid size-11 shrink-0 place-items-center rounded-2xl bg-accent text-accent-foreground *:size-5.5">
          {icon}
        </div>
        <span className="min-w-0 flex-1 truncate text-lg font-bold text-foreground">{title}</span>
        {badge}
      </div>
      {(description != null || children != null) && (
        <div className="flex flex-col gap-2">
          {description != null && (
            <p className="truncate text-sm font-semibold text-muted-foreground">{description}</p>
          )}
          {children}
        </div>
      )}
      <div className="mt-auto truncate pt-1 text-xs font-medium text-muted-foreground">
        {footer}
      </div>
    </Card>
  );
}
