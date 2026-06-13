import * as React from "react";

import { cn } from "@/lib/utils";

// animal-island-ui(guokaigdg)の Footer を移植。原典は背景画像だけの装飾帯
// (高さ 80px・全幅)。色や寸法は src/components/Footer/footer.module.less を
// 厳密に踏襲する。
//   - sea  : footer-sea.svg を center / contain no-repeat(原典の既定背景)
//   - tree : footer-tree.webp を cover / bottom center no-repeat で上書き
// 画像は web/public/footer/ に配置し /footer/<file> で参照する。

export type FooterType = "sea" | "tree";

export interface FooterProps {
  /** Footer 種類。sea=波の帯 / tree=樹冠の帯(既定) */
  type?: FooterType;
  /** 追加クラス名 */
  className?: string;
  /** 追加インラインスタイル */
  style?: React.CSSProperties;
}

// 共通:全幅・高さ 80px。背景は no-repeat。背景画像と寸法・位置は type で切り替える。
const BASE = "w-full h-[80px] bg-no-repeat";

const TYPE: Record<FooterType, string> = {
  // 原典 .footer:center / contain
  sea: "bg-[url('/footer/footer-sea.svg')] bg-center bg-contain",
  // 原典 .tree:cover / bottom center(.footer の no-repeat を継承)
  tree: "bg-[url('/footer/footer-tree.webp')] bg-bottom bg-cover",
};

export function Footer({ type = "tree", className, style }: FooterProps) {
  // 背景画像だけの装飾帯。意味のあるコンテンツを持たないので AT からは隠す。
  return <div aria-hidden="true" className={cn(BASE, TYPE[type], className)} style={style} />;
}
