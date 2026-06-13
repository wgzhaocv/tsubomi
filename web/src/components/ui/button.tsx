import * as React from "react";
import { Slot } from "@radix-ui/react-slot";

import { cn } from "@/lib/utils";

// animal-island-ui(guokaigdg)の Button を移植。原典は prop 式 API
// (type/danger/ghost/loading/block/size/icon)。色・寸法・3D 影は
// src/components/Button/button.module.less と variables.less を厳密に踏襲。
// tsubomi 拡張:asChild(Slot)で <a>/<Link> をボタン外見でレンダーできる。

export type ButtonType = "primary" | "default" | "dashed" | "text" | "link";
export type ButtonSize = "small" | "middle" | "large";

export interface ButtonProps extends Omit<React.ButtonHTMLAttributes<HTMLButtonElement>, "type"> {
  /** 種類。primary=クリーム立体 / default=枠付き / dashed=破線 / text=地味 / link=ミント文字 */
  type?: ButtonType;
  /** 寸法 */
  size?: ButtonSize;
  /** 危険(赤系) */
  danger?: boolean;
  /** ゴースト(透明・ミント枠) */
  ghost?: boolean;
  /** ブロック(全幅) */
  block?: boolean;
  /** ローディング(斜めストライプ + 操作不可) */
  loading?: boolean;
  /** 前置アイコン */
  icon?: React.ReactNode;
  /** ネイティブ button type */
  htmlType?: "submit" | "reset" | "button";
  /** 単一の子要素にスタイルを移譲(<a> 等をボタン外見に) */
  asChild?: boolean;
}

// focus は outline で表現する(原典どおり)。ring(box-shadow)だと 3D 影の
// box-shadow と衝突して消えるため使わない。
const BASE =
  "relative inline-flex shrink-0 cursor-pointer items-center justify-center gap-2 border-2 border-transparent font-semibold leading-none tracking-[0.02em] whitespace-nowrap shadow-[0_2px_4px_0_rgba(61,52,40,0.06)] transition-all duration-250 ease-in-out outline-none select-none focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-2 [&_svg]:pointer-events-none [&_svg:not([class*='size-'])]:size-4";

const SIZE: Record<ButtonSize, string> = {
  // small:32px / radius16、large:48px / radius24、middle:45px / 丸薬形(base の rounded-full)
  small: "h-8 rounded-2xl px-4 text-xs",
  middle: "h-11.25 rounded-full px-5 text-sm",
  large: "h-12 rounded-3xl px-8 text-base",
};

// small は視覚高さ 32px のままで、当たり判定だけを擬似要素で 44px まで広げる
// (WCAG 2.5.5 のタッチターゲット下限)。見た目の寸法は一切変えない。
// 擬似要素は中央に絶対配置し、ボタン自身を超えてはみ出させる(視覚影響なし)。
const SMALL_HIT_AREA =
  "before:absolute before:left-1/2 before:top-1/2 before:h-11 before:min-h-full before:w-full before:-translate-x-1/2 before:-translate-y-1/2 before:content-['']";

const LOADING =
  "pointer-events-none cursor-default border-4 border-[#4de2da] bg-[#0ec4b6] bg-size-[28.28px_28.28px] text-white shadow-none bg-[repeating-linear-gradient(-45deg,#0ec4b6,#0ec4b6_10px,#01b0a7_10px,#01b0a7_20px)] animate-[animal-btn-loading_1s_linear_infinite]";

// 種類ごとのクラス。hover/active は interactive(=非 disabled/非 loading)のときのみ付与する。
function typeClasses(
  type: ButtonType,
  danger: boolean,
  ghost: boolean,
  interactive: boolean,
): string {
  if (ghost) {
    // 原典は ghost+primary のみ定義。静止時は枠色を上書きしないので .btn-primary の
    // #f8f8f0(クリーム=ほぼ不可視)を継承し、文字だけミント。hover で枠/文字が
    // ミント(#3dd4c6)+ 淡ミント背景になる。
    return cn(
      "border-[#f8f8f0] bg-transparent text-[#19c8b9] shadow-none",
      interactive && "hover:border-[#3dd4c6] hover:bg-[rgba(25,200,185,0.08)] hover:text-[#3dd4c6]",
    );
  }
  if (danger) {
    if (type === "primary") {
      return cn(
        "border-[#e05a5a] bg-[#e05a5a] text-white shadow-[0_5px_0_0_#c94444]",
        interactive &&
          "hover:-translate-y-px hover:border-[#e87878] hover:bg-[#e87878] hover:shadow-[0_6px_0_0_#c94444] active:translate-y-0.5 active:border-[#c94444] active:bg-[#c94444] active:shadow-[0_1px_0_0_#c94444]",
      );
    }
    if (type === "default" || type === "dashed") {
      return cn(
        "border-[#e05a5a] bg-[#f8f8f0] text-[#e05a5a]",
        type === "dashed" && "border-dashed",
        interactive &&
          "hover:-translate-y-px hover:border-[#e87878] active:translate-y-0 active:border-[#c94444]",
      );
    }
    return "border-transparent bg-transparent text-[#e05a5a] shadow-none";
  }
  switch (type) {
    case "primary":
      return cn(
        "border-[#f8f8f0] bg-[#f8f8f0] text-[#794f27] shadow-[0_5px_0_0_#bdaea0]",
        interactive &&
          "hover:-translate-y-px hover:shadow-[0_6px_0_0_#bdaea0] active:translate-y-0.5 active:shadow-[0_1px_0_0_#bdaea0]",
      );
    case "dashed":
      return cn(
        "border-dashed border-[#aaa69d] bg-[#f8f8f0] text-[#794f27]",
        interactive &&
          "hover:-translate-y-px hover:border-[#19c8b9] hover:text-[#19c8b9] active:translate-y-0 active:border-[#50B9AB] active:text-[#50B9AB]",
      );
    case "text":
      return cn(
        "border-transparent bg-transparent text-[#794f27] shadow-none",
        interactive && "hover:bg-[#f0e8d8] active:bg-[#e8dcc4]",
      );
    case "link":
      return cn(
        "border-transparent bg-transparent text-[#19c8b9] shadow-none",
        interactive && "hover:text-[#3dd4c6] hover:opacity-85 active:text-[#50B9AB]",
      );
    case "default":
    default:
      return cn(
        "border-[#aaa69d] bg-[#f8f8f0] text-[#794f27]",
        interactive &&
          "hover:-translate-y-px hover:border-[#19c8b9] hover:text-[#19c8b9] hover:shadow-[0_3px_10px_0_rgba(61,52,40,0.10)] active:translate-y-0 active:border-[#50B9AB] active:text-[#50B9AB] active:shadow-[0_2px_4px_0_rgba(61,52,40,0.06)]",
      );
  }
}

export function Button({
  type = "default",
  size = "middle",
  danger = false,
  ghost = false,
  block = false,
  loading = false,
  disabled = false,
  icon,
  htmlType = "button",
  asChild = false,
  className,
  children,
  ...rest
}: ButtonProps) {
  const interactive = !disabled && !loading;
  // disabled も loading も「操作不可」。両者を 1 つの実効状態にまとめる。
  const isDisabled = disabled || loading;

  // P2:アイコンのみ(children 無し)のとき、アクセシブルネームが無いと
  // スクリーンリーダで無名ボタンになる。開発時だけ警告する(クラッシュはさせない)。
  if (import.meta.env.DEV && icon && children == null) {
    const hasName = "aria-label" in rest || "aria-labelledby" in rest;
    if (!hasName) {
      console.warn(
        "[Button] icon のみで children が無い場合は aria-label か aria-labelledby を指定してください(無名ボタンになります)。",
      );
    }
  }

  const cls = cn(
    BASE,
    SIZE[size],
    size === "small" && SMALL_HIT_AREA,
    loading ? LOADING : typeClasses(type, danger, ghost, interactive),
    // 淡色化(opacity)は「真の disabled」のときだけ。loading は満色のまま
    // (操作不可は native disabled / aria-busy で担保、見た目は LOADING が支配)。
    disabled && !loading && "cursor-not-allowed opacity-50 shadow-none",
    block && "flex w-full",
    className,
  );

  // asChild:単一の子(<a>/<Link> 等)へスタイル移譲。icon/loading 装飾は付けない。
  // P0:非 button の子は native disabled を持てないので、無効時は ARIA とフォーカス・
  // ポインタ操作を明示的に塞ぐ(子側が自前で処理しない前提の防御)。click も握り潰す。
  if (asChild) {
    const slotProps: React.HTMLAttributes<HTMLElement> = {
      ...(rest as React.HTMLAttributes<HTMLElement>),
    };
    if (isDisabled) {
      slotProps["aria-disabled"] = true;
      slotProps.tabIndex = -1;
      slotProps.className = cn(cls, "pointer-events-none");
      // キャプチャ段でクリックを止める(子の onClick / 既定遷移を発火させない)。
      slotProps.onClickCapture = (event) => {
        event.preventDefault();
        event.stopPropagation();
      };
    } else {
      slotProps.className = cls;
    }
    return <Slot {...slotProps}>{children}</Slot>;
  }

  return (
    <button
      type={htmlType}
      className={cls}
      disabled={isDisabled}
      aria-busy={loading || undefined}
      {...rest}
    >
      {icon && !loading && <span className="inline-flex items-center">{icon}</span>}
      {children != null && <span>{children}</span>}
    </button>
  );
}
