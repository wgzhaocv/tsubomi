import * as React from "react";

import { cn } from "@/lib/utils";

// animal-island-ui(guokaigdg)の Divider を移植。原典は prop 式 API(type)。
// 寸法・配色は src/components/Divider/divider.module.less を厳密に踏襲。
// line-*/wave-* は背景画像アセット(public/dividers/)を center/contain で敷く。
// dashed-* は CSS のみ:linear-gradient(to right, <hex> 50%, transparent 50%)
// を center / 12px 2px repeat-x で繰り返し、太さ 2px の破線を描く。

export type DividerType =
  | "line-brown"
  | "line-teal"
  | "line-white"
  | "line-yellow"
  | "wave-yellow"
  | "dashed-brown"
  | "dashed-teal"
  | "dashed-white"
  | "dashed-yellow";

export interface DividerProps {
  /** 分割線の種類(既定 line-brown) */
  type?: DividerType;
  /** 追加クラス */
  className?: string;
  /** 追加スタイル */
  style?: React.CSSProperties;
}

// 基底:幅 100% / 高さ 12px。画像 variant は中央寄せ contain・繰り返し無し。
const BASE = "w-full h-3 bg-center bg-contain bg-no-repeat";

// line-*/wave-*:背景画像アセット(.less の url() と同じファイルを参照)。
// dashed-*:画像無し。グラデーションで 12px 2px の破線リズムを repeat-x する
// (.less の `background: linear-gradient(...) center / 12px 2px repeat-x` 同値)。
const TYPE: Record<DividerType, string> = {
  "line-brown": "bg-[url(/dividers/divider-line-brown.svg)]",
  "line-teal": "bg-[url(/dividers/divider-line-teal.svg)]",
  "line-white": "bg-[url(/dividers/divider-line-white.png)]",
  "line-yellow": "bg-[url(/dividers/divider-line-yellow.svg)]",
  "wave-yellow": "bg-[url(/dividers/wave-yellow.svg)]",
  "dashed-brown":
    "bg-[length:12px_2px] bg-repeat-x [background-image:linear-gradient(to_right,#c4b89e_50%,transparent_50%)]",
  "dashed-teal":
    "bg-[length:12px_2px] bg-repeat-x [background-image:linear-gradient(to_right,#19c8b9_50%,transparent_50%)]",
  "dashed-white":
    "bg-[length:12px_2px] bg-repeat-x [background-image:linear-gradient(to_right,#ffffff_50%,transparent_50%)]",
  "dashed-yellow":
    "bg-[length:12px_2px] bg-repeat-x [background-image:linear-gradient(to_right,#f5d04a_50%,transparent_50%)]",
};

export function Divider({ type = "line-brown", className, style }: DividerProps) {
  // 意味上は区切り線なので role="separator"(既定で水平)。純装飾として使う場合は
  // 呼び出し側で aria-hidden を被せて上書きできる。
  return <div role="separator" className={cn(BASE, TYPE[type], className)} style={style} />;
}

Divider.displayName = "Divider";
