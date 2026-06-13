import * as React from "react";

import { cn } from "@/lib/utils";

// animal-island-ui の Checkbox を移植。円形ボックス + 白いチェック(SVG path を
// stroke-dashoffset で描く) + チェック時の放射スプラッシュ。色・寸法は
// checkbox.module.less / variables.less を厳密に踏襲。
// ※ チェックの描画は原典の `input:checked ~ .check path` を React state で再現する
//   (path は svg の子で input の兄弟ではないため peer-checked では届かない)。

export type CheckboxSize = "small" | "middle" | "large";

export interface CheckboxOption {
  label: React.ReactNode;
  value: string | number;
  disabled?: boolean;
}

export interface CheckboxProps {
  /** 選択値リスト(受控) */
  value?: Array<string | number>;
  /** 既定選択値リスト(非受控) */
  defaultValue?: Array<string | number>;
  options: CheckboxOption[];
  size?: CheckboxSize;
  /** 全体を無効化 */
  disabled?: boolean;
  direction?: "horizontal" | "vertical";
  onChange?: (values: Array<string | number>) => void;
  className?: string;
  style?: React.CSSProperties;
  /** group の読み上げ名(直接文字列)。"aria-labelledby" と併用しない */
  "aria-label"?: string;
  /** group の読み上げ名(別要素の id 参照) */
  "aria-labelledby"?: string;
}

// box=円の直径 / check=チェック SVG の幅高 / label=文字サイズ。
const SIZE: Record<CheckboxSize, { box: string; check: string; label: string }> = {
  small: { box: "size-[18px]", check: "h-[9px] w-[10px]", label: "text-xs" },
  middle: { box: "size-[22px]", check: "h-[11px] w-3", label: "text-sm" },
  large: { box: "size-7", check: "h-[14px] w-[15px]", label: "text-base" },
};

export function Checkbox({
  value,
  defaultValue = [],
  options,
  size = "middle",
  disabled = false,
  direction = "horizontal",
  onChange,
  className,
  style,
  "aria-label": ariaLabel,
  "aria-labelledby": ariaLabelledby,
}: CheckboxProps) {
  const [innerValue, setInnerValue] = React.useState<Array<string | number>>(defaultValue);
  const isControlled = value !== undefined;
  const checkedValues = isControlled ? value : innerValue;

  const handleChange = (optValue: string | number, optDisabled?: boolean) => {
    if (disabled || optDisabled) return;
    const next = checkedValues.includes(optValue)
      ? checkedValues.filter((v) => v !== optValue)
      : [...checkedValues, optValue];
    if (!isControlled) setInnerValue(next);
    onChange?.(next);
  };

  const sz = SIZE[size];

  return (
    <div
      role="group"
      aria-label={ariaLabel}
      aria-labelledby={ariaLabelledby}
      className={cn(
        "flex flex-wrap font-medium",
        direction === "vertical" ? "flex-col gap-3" : "flex-row gap-4",
        className,
      )}
      style={style}
    >
      {options.map((opt) => {
        const isChecked = checkedValues.includes(opt.value);
        const isDisabled = disabled || opt.disabled;
        return (
          <label
            key={String(opt.value)}
            className={cn(
              "relative inline-flex items-center gap-2 select-none",
              isDisabled ? "cursor-not-allowed opacity-55" : "cursor-pointer",
            )}
          >
            <span className={cn("relative shrink-0", sz.box)}>
              <input
                type="checkbox"
                checked={isChecked}
                disabled={isDisabled}
                onChange={() => {
                  handleChange(opt.value, opt.disabled);
                }}
                className={cn(
                  "absolute inset-0 m-0 cursor-pointer appearance-none rounded-full border-2 border-[#c4b89e] bg-[rgb(247,243,223)] outline-none transition-[border-color] duration-250",
                  "checked:border-[#50B9AB] checked:bg-[#19c8b9]",
                  "focus-visible:[outline:2px_solid_#f5c31c] focus-visible:outline-offset-2",
                  "disabled:border-[#d4c9b4] disabled:bg-[#f0ece2]",
                )}
              />
              {/* チェック時に放射状へ弾けるスプラッシュ */}
              <span
                aria-hidden
                className={cn(
                  "pointer-events-none absolute inset-0 block rounded-full",
                  isChecked && !isDisabled && "animate-[animal-cbx-splash_0.6s_ease_forwards]",
                )}
              />
              {/* 白いチェック:stroke-dashoffset を 0 へ流して描画 */}
              <svg
                className={cn(
                  "pointer-events-none absolute top-1/2 left-1/2 z-1 -translate-x-1/2 -translate-y-[54%]",
                  sz.check,
                )}
                viewBox="0 0 15 14"
                fill="none"
                aria-hidden
              >
                <path
                  d="M2 8.36364L6.23077 12L13 2"
                  stroke={isDisabled ? "#c4b89e" : "#fff"}
                  strokeWidth={3}
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  style={{
                    strokeDasharray: 19,
                    strokeDashoffset: isChecked ? 0 : 19,
                    transition: "stroke-dashoffset 0.3s ease 0.2s",
                  }}
                />
              </svg>
            </span>
            <span
              className={cn(
                "tracking-[0.01em] transition-colors",
                sz.label,
                isDisabled ? "text-[#c4b89e]" : isChecked ? "text-[#794f27]" : "text-[#725d42]",
              )}
            >
              {opt.label}
            </span>
          </label>
        );
      })}
    </div>
  );
}
