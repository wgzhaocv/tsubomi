import * as React from "react";

import { cn } from "@/lib/utils";

// animal-island-ui(guokaigdg)の Input を移植。原典どおり <span> ラッパに
// input + prefix/suffix/clear を収める prop 式 API。色・寸法・下方向の影は
// src/components/Input/input.module.less を厳密に踏襲(影は shadow=true のとき)。

export type InputSize = "small" | "middle" | "large";

export interface InputProps extends Omit<
  React.InputHTMLAttributes<HTMLInputElement>,
  "size" | "prefix"
> {
  /** 寸法 */
  size?: InputSize;
  /** input の id(未指定なら React.useId で自動生成し label と紐付ける) */
  id?: string;
  /** 可視ラベル(指定時は <label htmlFor> を input の上に描画) */
  label?: React.ReactNode;
  /** 補助説明文(フィールド下に描画し aria-describedby で結ぶ) */
  description?: React.ReactNode;
  /** エラーメッセージ(フィールド下に描画し aria-describedby で結ぶ) */
  errorMessage?: React.ReactNode;
  /** 前置アイコン */
  prefix?: React.ReactNode;
  /** 後置アイコン */
  suffix?: React.ReactNode;
  /** クリアボタンを表示 */
  allowClear?: boolean;
  /** 状態色 */
  status?: "error" | "warning";
  /** 下方向の立体影を出す(原典の既定は false) */
  shadow?: boolean;
  /** クリア時のコールバック */
  onClear?: () => void;
  /** クリアボタンの aria-label */
  clearAriaLabel?: string;
}

const WRAP_BASE =
  "inline-flex w-full items-center rounded-full border-2 border-[#c4b89e] bg-[rgb(247,243,223)] transition-all duration-250 ease-in-out";

const WRAP_SIZE: Record<InputSize, string> = {
  small: "h-8 rounded-[40px] px-[14px] text-xs",
  middle: "h-10 px-[18px] text-sm",
  large: "h-12 rounded-full border-[2.5px] px-[22px] text-base",
};

// 影あり時のサイズ別オフセット(small=2px / middle=3px / large=4px)。
const WRAP_SHADOW: Record<InputSize, string> = {
  small: "shadow-[0_2px_0_0_#d4c9b4] hover:shadow-[0_2px_0_0_#c4b89e]",
  middle: "shadow-[0_3px_0_0_#d4c9b4] hover:shadow-[0_3px_0_0_#c4b89e]",
  large: "shadow-[0_4px_0_0_#d4c9b4] hover:shadow-[0_4px_0_0_#c4b89e]",
};

const WRAP_STATUS = {
  error:
    "border-[#e05a5a] shadow-[0_3px_0_0_#c94444] hover:border-[#e87878] hover:shadow-[0_3px_0_0_#c94444]",
  warning:
    "border-[#f5c31c] shadow-[0_3px_0_0_#dba90e] hover:border-[#f7d04a] hover:shadow-[0_3px_0_0_#dba90e]",
} as const;

export function Input({
  size = "middle",
  id,
  label,
  description,
  errorMessage,
  prefix,
  suffix,
  allowClear = false,
  status,
  shadow = false,
  disabled = false,
  className,
  value,
  defaultValue,
  onChange,
  onClear,
  clearAriaLabel = "クリア",
  ...rest
}: InputProps) {
  const [innerValue, setInnerValue] = React.useState(defaultValue ?? "");
  const isControlled = value !== undefined;
  const currentValue = isControlled ? value : innerValue;

  // id 未指定なら自動生成し、label / 説明文 / エラー文と紐付ける
  const reactId = React.useId();
  const inputId = id ?? reactId;
  const descriptionId = `${inputId}-description`;
  const errorId = `${inputId}-error`;

  const handleChange: React.ChangeEventHandler<HTMLInputElement> = (e) => {
    if (!isControlled) setInnerValue(e.target.value);
    onChange?.(e);
  };

  const handleClear = () => {
    if (!isControlled) setInnerValue("");
    onClear?.();
  };

  const hasValue = String(currentValue ?? "").length > 0;

  // aria-describedby は存在するものだけを結ぶ。エラー文は優先(末尾に追加)。
  const describedBy =
    [description ? descriptionId : null, errorMessage ? errorId : null].filter(Boolean).join(" ") ||
    undefined;

  return (
    <div className="flex w-full flex-col gap-1">
      {label && (
        <label htmlFor={inputId} className="text-sm font-medium text-[#725d42]">
          {label}
        </label>
      )}
      {/* フィールド本体のラッパ(見た目・寸法は原典どおり) */}
      <span
        className={cn(
          WRAP_BASE,
          WRAP_SIZE[size],
          // 通常時の hover 枠色(状態色・disabled が無いとき)
          !status && !disabled && "hover:border-[#a89878]",
          shadow && !status && !disabled && WRAP_SHADOW[size],
          status && WRAP_STATUS[status],
          // 内部 input が focus-visible のとき、ラッパに焦点アウトライン(黄)を出す
          "has-[:focus-visible]:[outline:2px_solid_#f5c31c] has-[:focus-visible]:[outline-offset:2px]",
          disabled && "cursor-not-allowed border-[#d4c9b4] bg-[#ece8dc] opacity-60 shadow-none",
          className,
        )}
      >
        {prefix && (
          <span aria-hidden className="mr-1.5 inline-flex shrink-0 items-center text-[#a0936e]">
            {prefix}
          </span>
        )}
        <input
          id={inputId}
          className="w-full flex-1 border-none bg-transparent font-medium tracking-[0.01em] text-[#725d42] outline-none placeholder:font-normal placeholder:text-[#c4b89e] disabled:cursor-not-allowed disabled:text-[#c4b89e]"
          disabled={disabled}
          value={currentValue}
          onChange={handleChange}
          aria-invalid={status === "error" ? true : undefined}
          aria-describedby={describedBy}
          {...rest}
        />
        {allowClear && hasValue && !disabled && (
          <button
            type="button"
            onClick={handleClear}
            aria-label={clearAriaLabel}
            // 可視グリフは 20px のまま。before 疑似要素で当たり判定だけを
            // 上下左右 -12px 拡張し 44px のタッチ領域を確保(レイアウトは不変)。
            className="relative ml-1 inline-flex size-5 shrink-0 items-center justify-center rounded-full border-none bg-transparent text-[13px] font-bold text-[#c4b89e] outline-none transition-colors duration-150 before:absolute before:-inset-3 before:content-[''] hover:bg-[rgba(114,93,66,0.1)] hover:text-[#725d42] focus-visible:[outline:2px_solid_#725d42] focus-visible:[outline-offset:1px]"
          >
            ×
          </button>
        )}
        {suffix && (
          <span aria-hidden className="ml-1.5 inline-flex shrink-0 items-center text-[#a0936e]">
            {suffix}
          </span>
        )}
      </span>
      {description && (
        <span id={descriptionId} className="text-xs text-[#a0936e]">
          {description}
        </span>
      )}
      {errorMessage && (
        <span id={errorId} className="text-xs text-[#e05a5a]">
          {errorMessage}
        </span>
      )}
    </div>
  );
}
