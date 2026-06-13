import * as React from "react";

import { cn } from "@/lib/utils";

// animal-island-ui の Radio を移植。原典は options 配列を受け取るラジオ群。
// 見た目は Checkbox と同じ円形ボックス + 白いチェック(原典は radio も checkmark)
// + 選択時の放射スプラッシュ。単一選択 + roving tabindex のキーボード操作を踏襲。
// 色・寸法は radio.module.less / variables.less に準拠。

export type RadioSize = "small" | "middle" | "large";

export interface RadioOption {
  label: React.ReactNode;
  value: string | number;
  disabled?: boolean;
}

export interface RadioProps {
  /** 選択値(受控) */
  value?: string | number;
  /** 初期選択値(非受控) */
  defaultValue?: string | number;
  options: RadioOption[];
  size?: RadioSize;
  /** 全体を無効化 */
  disabled?: boolean;
  direction?: "horizontal" | "vertical";
  onChange?: (value: string | number) => void;
  className?: string;
  style?: React.CSSProperties;
  /** radiogroup の読み上げ名(直接文字列)。"aria-labelledby" と併用しない */
  "aria-label"?: string;
  /** radiogroup の読み上げ名(別要素の id 参照) */
  "aria-labelledby"?: string;
}

const SIZE: Record<RadioSize, { box: string; check: string; label: string }> = {
  small: { box: "size-[18px]", check: "h-[9px] w-[10px]", label: "text-xs" },
  middle: { box: "size-[22px]", check: "h-[11px] w-3", label: "text-sm" },
  large: { box: "size-7", check: "h-[14px] w-[15px]", label: "text-base" },
};

export function Radio({
  value,
  defaultValue,
  options,
  size = "middle",
  disabled = false,
  direction = "horizontal",
  onChange,
  className,
  style,
  "aria-label": ariaLabel,
  "aria-labelledby": ariaLabelledby,
}: RadioProps) {
  const [innerValue, setInnerValue] = React.useState<string | number | undefined>(defaultValue);
  const isControlled = value !== undefined;
  const checkedValue = isControlled ? value : innerValue;

  const reactId = React.useId();
  const idBase = `tbm-radio-${reactId.replace(/:/g, "")}`;
  const inputRefs = React.useRef<Array<HTMLInputElement | null>>([]);

  const enabledIndices = React.useMemo(
    () =>
      options
        .map((opt, idx) => ({ opt, idx }))
        .filter(({ opt }) => !disabled && !opt.disabled)
        .map(({ idx }) => idx),
    [options, disabled],
  );

  // roving tabindex の初期フォーカス索引を決める:選択済みかつ有効なら其れ、
  // さもなくば最初の「有効」オプション。無効を初期値にすると tabIndex=0 が
  // どの enabled にも付かず群が Tab で到達不能になる(P0)。0 で固定しない。
  const resolveInitialFocus = React.useCallback(() => {
    const checkedIdx = options.findIndex((o) => o.value === checkedValue);
    if (checkedIdx >= 0 && enabledIndices.includes(checkedIdx)) return checkedIdx;
    return enabledIndices.length > 0 ? enabledIndices[0] : 0;
  }, [options, checkedValue, enabledIndices]);

  // roving tabindex の現在フォーカス索引
  const [focusedIndex, setFocusedIndex] = React.useState<number>(resolveInitialFocus);

  // 選択値・options・disabled の変化で再計算。focusedIndex が無効を指して
  // しまった場合も有効な索引へ寄せ直す(group を常に Tab 到達可能に保つ)。
  React.useEffect(() => {
    setFocusedIndex((prev) => {
      const checkedIdx = options.findIndex((o) => o.value === checkedValue);
      if (checkedIdx >= 0 && enabledIndices.includes(checkedIdx)) return checkedIdx;
      if (enabledIndices.includes(prev)) return prev;
      return enabledIndices.length > 0 ? enabledIndices[0] : 0;
    });
  }, [checkedValue, options, enabledIndices]);

  const currentEnabledPos = enabledIndices.indexOf(focusedIndex);

  const handleChange = (optValue: string | number, optDisabled?: boolean) => {
    if (disabled || optDisabled) return;
    if (!isControlled) setInnerValue(optValue);
    onChange?.(optValue);
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (enabledIndices.length === 0) return;
    let nextPos = -1;
    switch (e.key) {
      case "ArrowRight":
      case "ArrowDown":
        e.preventDefault();
        nextPos = (currentEnabledPos + 1) % enabledIndices.length;
        break;
      case "ArrowLeft":
      case "ArrowUp":
        e.preventDefault();
        nextPos = (currentEnabledPos - 1 + enabledIndices.length) % enabledIndices.length;
        break;
      case "Home":
        e.preventDefault();
        nextPos = 0;
        break;
      case "End":
        e.preventDefault();
        nextPos = enabledIndices.length - 1;
        break;
      default:
        return;
    }
    if (nextPos >= 0) {
      const nextIdx = enabledIndices[nextPos];
      setFocusedIndex(nextIdx);
      handleChange(options[nextIdx].value, options[nextIdx].disabled);
      inputRefs.current[nextIdx]?.focus();
    }
  };

  const sz = SIZE[size];

  return (
    <div
      className={cn(
        "flex flex-wrap font-medium",
        direction === "vertical" ? "flex-col gap-3" : "flex-row gap-4",
        className,
      )}
      style={style}
      role="radiogroup"
      aria-label={ariaLabel}
      aria-labelledby={ariaLabelledby}
      onKeyDown={handleKeyDown}
    >
      {options.map((opt, idx) => {
        const isChecked = checkedValue === opt.value;
        const isDisabled = disabled || opt.disabled;
        const isFocusable = idx === focusedIndex && !isDisabled;
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
                ref={(el) => {
                  inputRefs.current[idx] = el;
                }}
                type="radio"
                name={idBase}
                checked={isChecked}
                disabled={isDisabled}
                tabIndex={isFocusable ? 0 : -1}
                onChange={() => {
                  handleChange(opt.value, opt.disabled);
                }}
                onFocus={() => {
                  if (!isDisabled) setFocusedIndex(idx);
                }}
                className={cn(
                  "absolute inset-0 m-0 cursor-pointer appearance-none rounded-full border-2 border-[#c4b89e] bg-[rgb(247,243,223)] outline-none transition-[border-color] duration-250",
                  "checked:border-[#50B9AB] checked:bg-[#19c8b9]",
                  "focus-visible:[outline:2px_solid_#f5c31c] focus-visible:outline-offset-2",
                  "disabled:border-[#d4c9b4] disabled:bg-[#f0ece2]",
                )}
              />
              <span
                aria-hidden
                className={cn(
                  "pointer-events-none absolute inset-0 block rounded-full",
                  isChecked && !isDisabled && "animate-[animal-radio-splash_0.6s_ease_forwards]",
                )}
              />
              <svg
                className={cn(
                  "pointer-events-none absolute top-1/2 left-1/2 z-1 -translate-x-1/2 translate-y-[-54%]",
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
