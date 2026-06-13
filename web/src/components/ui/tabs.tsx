import * as React from "react";

import { cn } from "@/lib/utils";

// animal-island-ui(guokaigdg)の Tabs を移植。原典は items 配列駆動の prop 式 API
// (items/activeKey/defaultActiveKey/onChange)。色・寸法・影・葉っぱの揺れは
// src/components/Tabs/tabs.module.less と variables.less を厳密に踏襲。
// 葉っぱは原典そのものの icon-leaf.png(/icons に自托管)を <img> で出す。
// 揺れ keyframes(animal-leaf-wiggle)/内容フェード(animal-tab-fade-in)は
// グローバル CSS。制御/非制御の両対応・WAI-ARIA tablist 準拠。

export interface TabItem {
  key: string;
  /** タブ見出し */
  label: React.ReactNode;
  /** タブ本文 */
  children: React.ReactNode;
}

export interface TabsProps {
  items: TabItem[];
  /** 非制御時の初期 active key */
  defaultActiveKey?: string;
  /** 制御時の active key */
  activeKey?: string;
  /** active 変更時のコールバック */
  onChange?: (key: string) => void;
  className?: string;
  style?: React.CSSProperties;
  /** active タブの葉っぱを揺らす(原典の既定は true) */
  leafAnimation?: boolean;
  /** active タブに下方向の立体影を出す(原典の既定は true) */
  shadow?: boolean;
  /** 可視見出しが無いとき tablist に与える無障害ラベル */
  "aria-label"?: string;
}

export function Tabs({
  items,
  defaultActiveKey,
  activeKey,
  onChange,
  className,
  style,
  leafAnimation = true,
  shadow = true,
  "aria-label": ariaLabel,
}: TabsProps) {
  const [internalActiveKey, setInternalActiveKey] = React.useState(
    defaultActiveKey ?? items[0]?.key,
  );

  const currentActiveKey = activeKey !== undefined ? activeKey : internalActiveKey;

  // tablist 内の各 tab に安定 id 前缀を与え、aria-controls / aria-labelledby を双方向に紐づける
  const idPrefix = `animal-tabs-${React.useId().replace(/:/g, "")}`;
  const tabId = (k: string) => `${idPrefix}-tab-${k}`;
  const panelId = (k: string) => `${idPrefix}-panel-${k}`;

  const tabRefs = React.useRef<Map<string, HTMLButtonElement>>(new Map());

  const handleTabClick = React.useCallback(
    (key: string) => {
      if (activeKey === undefined) {
        setInternalActiveKey(key);
      }
      onChange?.(key);
    },
    [activeKey, onChange],
  );

  const focusTab = (key: string) => {
    tabRefs.current.get(key)?.focus();
  };

  // roving tabindex:矢印キー / Home / End で active を移しフォーカスを運ぶ(末尾↔先頭は循環)
  const handleKeyDown = React.useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      const { key } = e;
      if (key !== "ArrowRight" && key !== "ArrowLeft" && key !== "Home" && key !== "End") {
        return;
      }
      e.preventDefault();
      const idx = items.findIndex((i) => i.key === currentActiveKey);
      if (idx < 0) return;
      let nextIdx = idx;
      if (key === "ArrowRight") nextIdx = (idx + 1) % items.length;
      else if (key === "ArrowLeft") nextIdx = (idx - 1 + items.length) % items.length;
      else if (key === "Home") nextIdx = 0;
      else if (key === "End") nextIdx = items.length - 1;
      const nextKey = items[nextIdx].key;
      handleTabClick(nextKey);
      focusTab(nextKey);
    },
    [items, currentActiveKey, handleTabClick],
  );

  const activeItem = items.find((item) => item.key === currentActiveKey);

  return (
    // .tabs:暖クリーム面・丸み 24px・淡い枠・はみ出し隠し
    <div
      className={cn(
        "overflow-hidden rounded-3xl border-2 border-[#e8e2d6] bg-[#f8f8f0]",
        className,
      )}
      style={style}
    >
      {/* .tabList:半透明白の帯・下に淡い区切り線 */}
      <div
        className="flex gap-1 border-b-2 border-[#e8e2d6] bg-[rgba(255,255,255,0.6)] p-4"
        role="tablist"
        aria-label={ariaLabel}
        aria-orientation="horizontal"
        onKeyDown={handleKeyDown}
      >
        {items.map((item) => {
          const isActive = item.key === currentActiveKey;
          return (
            <button
              key={item.key}
              ref={(el) => {
                if (el) tabRefs.current.set(item.key, el);
                else tabRefs.current.delete(item.key);
              }}
              id={tabId(item.key)}
              type="button"
              role="tab"
              aria-selected={isActive}
              aria-controls={panelId(item.key)}
              tabIndex={isActive ? 0 : -1}
              onClick={() => handleTabClick(item.key)}
              className={cn(
                // .tabItem:透明地・枠なし・丸み 24px・茶文字 500
                "relative flex cursor-pointer items-center gap-2 rounded-3xl border-none bg-transparent px-4 py-2 text-sm font-medium text-[#794f27] outline-none transition-all duration-250 ease-in-out focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-2",
                // hover:ミント薄掛け(active 時は active 色が勝つ)
                !isActive && "hover:bg-[rgba(25,200,185,0.1)]",
                // .active:ミント面・クリーム文字 600
                isActive && "bg-[#0CC0B5] font-semibold text-[#FFF9E3]",
                // .active-shadow:下方向の立体影
                isActive && shadow && "shadow-[0_3px_0_0_rgba(61,52,40,0.08)]",
              )}
            >
              {/* .tabIcon:active で 1.2 倍に膨らむ丸印 */}
              <span
                aria-hidden
                className={cn(
                  "text-[10px] transition-transform duration-250 ease-in-out",
                  isActive && "scale-[1.2]",
                )}
              >
                {isActive ? "●" : "○"}
              </span>
              {/* .tabLabel:active でクリーム文字 */}
              <span className={cn("relative", isActive && "text-[#FFF9E3]")}>{item.label}</span>
              {/* .tabLeaf:active タブ右上に原典の葉っぱ(icon-leaf.png)。揺れは leafAnimation 任意 */}
              {isActive && (
                <img
                  src="/icons/icon-leaf.png"
                  alt=""
                  aria-hidden
                  className={cn(
                    "absolute -top-1 -right-[5px] size-[18px]",
                    leafAnimation && "animate-[animal-leaf-wiggle_2s_ease-in-out_infinite]",
                  )}
                />
              )}
            </button>
          );
        })}
      </div>
      {/* .tabContent:本文。表示の度に淡く立ち上がる(animal-tab-fade-in) */}
      <div
        className="min-h-[60px] animate-[animal-tab-fade-in_0.25s_ease] p-6"
        role="tabpanel"
        id={activeItem ? panelId(activeItem.key) : undefined}
        aria-labelledby={activeItem ? tabId(activeItem.key) : undefined}
        tabIndex={0}
      >
        {/* .tabContentInner:二次テキスト色・基本サイズ・基本行間 */}
        <div className="min-h-[40px] text-sm leading-[1.5715] text-[#9f927d]">
          {activeItem?.children}
        </div>
      </div>
    </div>
  );
}

Tabs.displayName = "Tabs";
