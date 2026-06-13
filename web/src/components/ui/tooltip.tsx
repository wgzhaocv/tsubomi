import * as React from "react";

import { cn } from "@/lib/utils";

// animal-island-ui の Tooltip を移植。原典は prop 式 API
// (title/placement/trigger/variant/bordered/children)で、hover/focus/click の
// 表示制御 + 12 方位の配置 + 矢印(default)/しっぽ(island)を持つ。色・寸法・
// モーションは src/components/Tooltip/tooltip.module.less と variables.less を
// 厳密に踏襲(@var はリテラルへ解決)。原典の CSS Modules は使えないので、
// 矢印・しっぽの幾何だけ tbm-tooltip- プレフィックスのグローバル CSS に逃がす。
// island の有機気泡は原典の clip-path / SVG をそのまま移植する。

export type TooltipPlacement =
  | "top"
  | "top-start"
  | "top-end"
  | "bottom"
  | "bottom-start"
  | "bottom-end"
  | "left"
  | "left-start"
  | "left-end"
  | "right"
  | "right-start"
  | "right-end";

export type TooltipTrigger = "hover" | "focus" | "click";

/** default 標準矩形;island 動森不規則有機気泡(Modal と同款 clip-path) */
export type TooltipVariant = "default" | "island";

// a11y 注意:tooltip の中身(title)は必ず「非インタラクティブ」であること。
// role="tooltip" は内部にフォーカス可能・操作可能な要素(button/link/input 等)を
// 持てない — スクリーンリーダーの読み上げ対象であって到達可能なウィジェットでは
// ないため、中にボタンやリンクを置いてもキーボード/支援技術から操作できない。
// インタラクティブな内容(ボタンを含むカード等)が必要なら、tooltip ではなく
// 別途 Popover コンポーネント(role="dialog" 等、フォーカス管理付き)を使うこと。

const ISLAND_CLIP_PATH =
  "M0.501,0.005 L0.501,0.005 L0.523,0.005 L0.549,0.006 C0.704,0.01,0.796,0.017,0.825,0.027 L0.827,0.028 C0.872,0.045,0.939,0.044,0.978,0.17 C1,0.254,1,0.365,0.99,0.505 L0.988,0.513 C0.979,0.558,0.971,0.598,0.965,0.633 C0.956,0.689,0.979,0.77,0.964,0.865 C0.953,0.928,0.921,0.966,0.869,0.979 C0.821,0.986,0.773,0.992,0.726,0.995 L0.712,0.996 L0.694,0.997 C0.648,1,0.586,1,0.507,1 L0.501,1 L0.464,1 C0.385,1,0.325,0.998,0.283,0.995 C0.234,0.992,0.184,0.987,0.133,0.979 C0.081,0.966,0.05,0.928,0.039,0.865 C0.023,0.77,0.047,0.689,0.037,0.633 C0.031,0.595,0.023,0.552,0.013,0.505 C-0.006,0.365,-0.002,0.254,0.024,0.17 C0.064,0.045,0.13,0.045,0.174,0.028 L0.175,0.028 C0.204,0.017,0.303,0.009,0.474,0.005 L0.501,0.005";

const ISLAND_BG = "rgb(247, 243, 223)";
const ISLAND_STROKE = "#c4b89e";

const IslandClipDef: React.FC<{ id: string }> = ({ id }) => (
  <svg style={{ position: "absolute", width: 0, height: 0 }} aria-hidden>
    <clipPath id={id} clipPathUnits="objectBoundingBox">
      <path d={ISLAND_CLIP_PATH} />
    </clipPath>
  </svg>
);

/** SVG が有機パスに沿って fill + stroke、枠線が不規則輪郭に貼り付く */
const IslandShapeSvg: React.FC = () => (
  <svg
    className="pointer-events-none absolute inset-0 z-0 block h-full w-full"
    viewBox="0 0 1 1"
    preserveAspectRatio="none"
    aria-hidden
  >
    <path
      d={ISLAND_CLIP_PATH}
      fill={ISLAND_BG}
      stroke={ISLAND_STROKE}
      strokeWidth={2}
      vectorEffect="non-scaling-stroke"
      strokeLinejoin="round"
    />
  </svg>
);

export interface TooltipProps {
  /** 提示内容、複数行対応(\n または <br/> で改行) */
  title: React.ReactNode;
  /** 位置 */
  placement?: TooltipPlacement;
  /** 触発方式 */
  trigger?: TooltipTrigger;
  /** 視覚スタイル:default 標準矩形 / island 動森有機気泡 */
  variant?: TooltipVariant;
  /** 枠線を表示するか(矢印描画含む);island では SVG が有機パスに沿って描画 */
  bordered?: boolean;
  /** 子要素(触発器) */
  children: React.ReactElement;
  /** 自定義クラス名 */
  className?: string;
  /** 自定義スタイル */
  style?: React.CSSProperties;
}

// placement → 配置クラス。bottom:calc(100%+10px) 等の位置決めと、表示前の
// 4px ずれ(matched in &.visible で 0 に戻す)を Tailwind 任意値で再現する。
// transform は visible 状態を data-visible で切り替える(原典の .visible 相当)。
const PLACEMENT: Record<TooltipPlacement, string> = {
  top: "bottom-[calc(100%+10px)] left-1/2 -translate-x-1/2 translate-y-[4px] data-[visible=true]:translate-y-0",
  "top-start":
    "bottom-[calc(100%+10px)] left-0 translate-y-[4px] data-[visible=true]:translate-y-0",
  "top-end": "bottom-[calc(100%+10px)] right-0 translate-y-[4px] data-[visible=true]:translate-y-0",
  bottom:
    "top-[calc(100%+10px)] left-1/2 -translate-x-1/2 translate-y-[-4px] data-[visible=true]:translate-y-0",
  "bottom-start":
    "top-[calc(100%+10px)] left-0 translate-y-[-4px] data-[visible=true]:translate-y-0",
  "bottom-end":
    "top-[calc(100%+10px)] right-0 translate-y-[-4px] data-[visible=true]:translate-y-0",
  left: "right-[calc(100%+10px)] top-1/2 -translate-y-1/2 translate-x-[4px] data-[visible=true]:translate-x-0",
  "left-start": "right-[calc(100%+10px)] top-0 translate-x-[4px] data-[visible=true]:translate-x-0",
  "left-end":
    "right-[calc(100%+10px)] bottom-0 translate-x-[4px] data-[visible=true]:translate-x-0",
  right:
    "left-[calc(100%+10px)] top-1/2 -translate-y-1/2 translate-x-[-4px] data-[visible=true]:translate-x-0",
  "right-start":
    "left-[calc(100%+10px)] top-0 translate-x-[-4px] data-[visible=true]:translate-x-0",
  "right-end":
    "left-[calc(100%+10px)] bottom-0 translate-x-[-4px] data-[visible=true]:translate-x-0",
};

// 矢印(default の ::after) / しっぽ(island の .tail)の方位別クラス。位置決めと
// 回転・方向別 border は tbm-tooltip- プレフィックスのグローバル CSS 側に持つ。
const placementKey = (p: TooltipPlacement) => p.replace(/-/g, "_");

export const Tooltip: React.FC<TooltipProps> = ({
  title,
  placement = "top",
  trigger = "hover",
  variant = "default",
  bordered = true,
  children,
  className,
  style,
}) => {
  const [visible, setVisible] = React.useState(false);
  const timerRef = React.useRef<ReturnType<typeof setTimeout>>(undefined);
  // click trigger の外側クリック判定用に、ラッパ span 全体(触発器 + tooltip)を参照する。
  const rootRef = React.useRef<HTMLSpanElement>(null);
  const rawId = React.useId().replace(/:/g, "");
  const clipId = `tbm-tooltip-clip-${rawId}`;
  const tooltipId = `tbm-tooltip-${rawId}`;

  const show = React.useCallback(() => {
    clearTimeout(timerRef.current);
    setVisible(true);
  }, []);

  const hide = React.useCallback(() => {
    timerRef.current = setTimeout(() => setVisible(false), 100);
  }, []);

  React.useEffect(() => () => clearTimeout(timerRef.current), []);

  // click trigger かつ表示中のみ:Escape で閉じる + 外側クリックで閉じる。
  // 非表示時や hover/focus trigger ではリスナを張らない(無駄な購読を避ける)。
  React.useEffect(() => {
    if (trigger !== "click" || !visible) return;

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setVisible(false);
    };
    const onPointerDown = (e: PointerEvent) => {
      const root = rootRef.current;
      if (root && e.target instanceof Node && !root.contains(e.target)) {
        setVisible(false);
      }
    };

    document.addEventListener("keydown", onKeyDown);
    document.addEventListener("pointerdown", onPointerDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("pointerdown", onPointerDown);
    };
  }, [trigger, visible]);

  const child = React.Children.only(children);
  const childProps = child.props as {
    "aria-describedby"?: string;
    onMouseEnter?: (e: React.MouseEvent) => void;
    onMouseLeave?: (e: React.MouseEvent) => void;
    onFocus?: (e: React.FocusEvent) => void;
    onBlur?: (e: React.FocusEvent) => void;
    onClick?: (e: React.MouseEvent) => void;
  };

  const triggerProps: Record<string, unknown> = {
    // スクリーンリーダーが trigger の focus / hover 時に tooltip 内容を読めるよう、
    // 表示中のみ aria-describedby を付与する(隠れた tooltip は読ませない)。
    "aria-describedby": visible
      ? [childProps["aria-describedby"], tooltipId].filter(Boolean).join(" ")
      : childProps["aria-describedby"],
  };

  if (trigger === "hover") {
    triggerProps.onMouseEnter = (e: React.MouseEvent) => {
      show();
      childProps.onMouseEnter?.(e);
    };
    triggerProps.onMouseLeave = (e: React.MouseEvent) => {
      hide();
      childProps.onMouseLeave?.(e);
    };
    // a11y:hover はポインタ専用でキーボード利用者に届かない。WCAG/WAI-ARIA の
    // tooltip パターンに従い、focus でも同じ表示/非表示を行う(focusin で出し、
    // focusout で隠す)。マウス hover はそのまま機能し、focus と独立に動く。
    triggerProps.onFocus = (e: React.FocusEvent) => {
      show();
      childProps.onFocus?.(e);
    };
    triggerProps.onBlur = (e: React.FocusEvent) => {
      hide();
      childProps.onBlur?.(e);
    };
  } else if (trigger === "focus") {
    triggerProps.onFocus = (e: React.FocusEvent) => {
      show();
      childProps.onFocus?.(e);
    };
    triggerProps.onBlur = (e: React.FocusEvent) => {
      hide();
      childProps.onBlur?.(e);
    };
  } else if (trigger === "click") {
    triggerProps.onClick = (e: React.MouseEvent) => {
      setVisible((v) => !v);
      childProps.onClick?.(e);
    };
    // a11y:click は開閉トグル(ディスクロージャ相当)。展開状態を触発器に
    // aria-expanded で公開し、内容との関連は aria-describedby が担う。
    triggerProps["aria-expanded"] = visible;
  }

  const isIsland = variant === "island";
  const pKey = placementKey(placement);

  return (
    <span
      ref={rootRef}
      className={cn("relative inline-flex align-middle", className)}
      style={style}
    >
      {React.cloneElement(child, triggerProps)}
      <div
        // .tooltip 基底:max-content + 240px 上限、6px/12px 余白、クリーム面、
        // radius16、@shadow-base、#725d42 文字、12px/500、opacity トランジション。
        className={cn(
          "absolute z-100 box-border w-max max-w-60 rounded-2xl px-3 py-1.5 text-xs font-medium leading-normal tracking-[0.01em] text-[#725d42] opacity-0 shadow-[0_3px_10px_0_rgba(61,52,40,0.1)] transition-[opacity,transform,translate] duration-250 ease-in-out pointer-events-none data-[visible=true]:pointer-events-auto data-[visible=true]:opacity-100",
          PLACEMENT[placement],
          isIsland
            ? // island:背景・枠・影を外し余白 0、上限 280px。矢印 ::after は無し。
              "max-w-70 bg-transparent p-0 shadow-none"
            : cn(
                "bg-[rgb(247,243,223)]",
                // 矢印(::after)の幾何はグローバル CSS。bordered のときのみ枠線 + 矢印描線。
                "tbm-tooltip-arrow",
                `tbm-tooltip-arrow-${pKey}`,
                bordered
                  ? "border-2 border-[#c4b89e] tbm-tooltip-bordered"
                  : "tbm-tooltip-borderless",
              ),
        )}
        role="tooltip"
        id={tooltipId}
        aria-hidden={!visible}
        data-visible={visible}
        onMouseEnter={trigger === "hover" ? show : undefined}
        onMouseLeave={trigger === "hover" ? hide : undefined}
      >
        {isIsland ? (
          <>
            <div className="relative w-max max-w-70">
              <IslandClipDef id={clipId} />
              {bordered && <IslandShapeSvg />}
              <div
                // bordered は drop-shadow を islandBody 側、borderless は面色 +
                // drop-shadow を content 側に置く(原典の振り分けを踏襲)。
                className={cn(
                  "relative z-1 px-5 py-3",
                  bordered
                    ? "bg-transparent"
                    : "bg-[rgb(247,243,223)] filter-[drop-shadow(0_4px_14px_rgba(121,79,39,0.14))]",
                )}
                style={{ clipPath: `url(#${clipId})` }}
              >
                <div className="relative z-1 whitespace-pre-line wrap-break-word text-center font-semibold leading-[1.55]">
                  {title}
                </div>
              </div>
            </div>
            <span
              className={cn(
                "pointer-events-none absolute z-2",
                bordered
                  ? cn("tbm-tooltip-tail-diamond", `tbm-tooltip-tail-${pKey}`)
                  : cn("tbm-tooltip-tail-dot", `tbm-tooltip-tail-${pKey}`),
              )}
              aria-hidden
            />
          </>
        ) : (
          <div className="relative z-1 whitespace-pre-line wrap-break-word">{title}</div>
        )}
      </div>
    </span>
  );
};

Tooltip.displayName = "Tooltip";
