import * as React from "react";

import { cn } from "@/lib/utils";

// animal-island-ui(guokaigdg)の Title を移植。どうぶつの森風の飘带(リボン/
// バナー)。原典 src/components/Title/title.module.less を 1:1 で踏襲し、
// 多層 clip-path 構造をそのまま再現する。bespoke CSS は index.css に置く
// (.tbm-ribbon* + 13 色)。font-size を inline で注入し、em 単位が追従する。

export type TitleSize = "small" | "middle" | "large";

export type TitleColor =
  | "default"
  | "app-pink"
  | "purple"
  | "app-blue"
  | "app-yellow"
  | "app-orange"
  | "app-teal"
  | "app-green"
  | "app-red"
  | "lime-green"
  | "yellow-green"
  | "brown"
  | "warm-peach-pink";

export interface TitleProps {
  /** 見出しの内容 */
  children: React.ReactNode;
  /** 寸法。small=14 / middle=20 / large=28 px を inline font-size で注入 */
  size?: TitleSize;
  /** 配色。Card と同名の色板(13 値) */
  color?: TitleColor;
  /** 追加クラス名 */
  className?: string;
  /** 追加スタイル */
  style?: React.CSSProperties;
}

// 寸法 → px。inline font-size として注入し、em 基準のリボン寸法が追従する。
const SIZE_MAP: Record<TitleSize, number> = {
  small: 14,
  middle: 20,
  large: 28,
};

// 色 → 色クラス(default は base .tbm-ribbon の緑をそのまま使う)。
const COLOR_CLASS: Record<TitleColor, string | null> = {
  default: null,
  "app-pink": "tbm-ribbon-app-pink",
  purple: "tbm-ribbon-purple",
  "app-blue": "tbm-ribbon-app-blue",
  "app-yellow": "tbm-ribbon-app-yellow",
  "app-orange": "tbm-ribbon-app-orange",
  "app-teal": "tbm-ribbon-app-teal",
  "app-green": "tbm-ribbon-app-green",
  "app-red": "tbm-ribbon-app-red",
  "lime-green": "tbm-ribbon-lime-green",
  "yellow-green": "tbm-ribbon-yellow-green",
  brown: "tbm-ribbon-brown",
  "warm-peach-pink": "tbm-ribbon-warm-peach-pink",
};

export function Title({
  children,
  size = "middle",
  color = "default",
  className,
  style,
}: TitleProps) {
  return (
    // 6 つの span による層構造:背景燕尾(左右)+ 折角三角(左右)+ 正面 + 文字。
    // font-size は inline で注入し、各層の em 単位が一斉に追従する。
    <span
      className={cn("tbm-ribbon", COLOR_CLASS[color], className)}
      style={{ fontSize: `${SIZE_MAP[size]}px`, ...style }}
    >
      <span className="tbm-ribbon-back tbm-ribbon-back-left" aria-hidden />
      <span className="tbm-ribbon-back tbm-ribbon-back-right" aria-hidden />
      <span className="tbm-ribbon-fold tbm-ribbon-fold-left" aria-hidden />
      <span className="tbm-ribbon-fold tbm-ribbon-fold-right" aria-hidden />
      <span className="tbm-ribbon-front" aria-hidden />
      <span className="tbm-ribbon-text">{children}</span>
    </span>
  );
}

Title.displayName = "Title";
