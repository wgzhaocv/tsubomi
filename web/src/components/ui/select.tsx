import * as React from "react";

import { cn } from "@/lib/utils";

// animal-island-ui の Select を移植。白トリガ + 黄色(#FFEEA0)の横出しドロップダウン、
// option は中央寄せ、hover で原典の select-cursor.svg が左から滑り込み、選択項の
// 下に半透明の黄色 pillBar を敷く。色・寸法・配置ロジックは select.module.less /
// Select.tsx を厳密に踏襲(矢印は inline SVG、カーソルは自托管の svg)。

export type SelectOption = {
  key: string;
  label: string;
};

export interface SelectProps {
  options: SelectOption[];
  value: string;
  onChange: (key: string) => void;
  placeholder?: string;
  disabled?: boolean;
  // 可視ラベル。渡されたら <label> を描画し combobox を aria-labelledby で紐付ける。
  label?: React.ReactNode;
  // フォーム送信に参加させる名前。渡されたら hidden input を描画する。
  name?: string;
  "aria-label"?: string;
  "aria-labelledby"?: string;
}

export function Select({
  options,
  value,
  onChange,
  placeholder = "選択してください",
  disabled = false,
  label,
  name,
  "aria-label": ariaLabel,
  "aria-labelledby": ariaLabelledBy,
}: SelectProps) {
  const [open, setOpen] = React.useState(false);
  const [activeKey, setActiveKey] = React.useState<string | null>(null);
  const [hoveredKey, setHoveredKey] = React.useState<string | null>(null);
  const [dropdownStyle, setDropdownStyle] = React.useState<React.CSSProperties>({});
  const [mounted, setMounted] = React.useState(false);
  const wrapperRef = React.useRef<HTMLDivElement>(null);
  const triggerRef = React.useRef<HTMLDivElement>(null);
  const currentLabel = options.find((o) => o.key === value)?.label ?? placeholder;

  const idPrefix = `tbm-select-${React.useId().replace(/:/g, "")}`;
  const listboxId = `${idPrefix}-listbox`;
  const labelId = `${idPrefix}-label`;
  const optionId = (k: string) => `${idPrefix}-option-${k}`;

  // アクセシブル名の解決:可視 label を渡したら aria-labelledby をそれに向ける
  // (利用側が明示した aria-labelledby があればそちらを優先)。
  const resolvedLabelledBy = ariaLabelledBy ?? (label != null ? labelId : undefined);

  // 開発時のみ:アクセシブル名がどれも無ければ警告(クラッシュはしない)。
  React.useEffect(() => {
    if (import.meta.env.DEV && label == null && !ariaLabel && !ariaLabelledBy) {
      console.warn(
        "[Select] アクセシブル名がありません。label / aria-label / aria-labelledby のいずれかを指定してください。",
      );
    }
  }, [label, ariaLabel, ariaLabelledBy]);

  // typeahead:印字可能文字を ~600ms 蓄積し、先頭一致 option へ activeKey を飛ばす。
  const typeaheadRef = React.useRef<{ buffer: string; timer: number | null }>({
    buffer: "",
    timer: null,
  });

  // クリックアウトサイドで閉じる
  React.useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (wrapperRef.current && !wrapperRef.current.contains(e.target as Node)) {
        setOpen(false);
        setMounted(false);
      }
    };
    if (open) document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [open]);

  // 横出し位置を空間に応じて算出(原典の配置ロジックそのまま)
  React.useEffect(() => {
    if (open && wrapperRef.current) {
      const rect = wrapperRef.current.getBoundingClientRect();
      const viewportWidth = window.innerWidth;
      const viewportHeight = window.innerHeight;
      const dropdownHeight = options.length * 44 + 24;
      const s: React.CSSProperties = { position: "absolute" };

      if (rect.right + 200 > viewportWidth) {
        s.right = "100%";
        s.marginRight = "6px";
        s.left = "auto";
      } else {
        s.left = "100%";
        s.marginLeft = "6px";
        s.right = "auto";
      }

      const spaceBelow = viewportHeight - rect.bottom;
      const spaceAbove = rect.top;
      if (spaceBelow < dropdownHeight && spaceAbove > spaceBelow) {
        s.top = "auto";
        s.bottom = "100%";
        s.marginBottom = "6px";
      } else if (spaceBelow < dropdownHeight) {
        s.top = "100%";
        s.marginTop = "6px";
        s.bottom = "auto";
      } else if (rect.top < dropdownHeight) {
        s.top = "100%";
        s.marginTop = "6px";
        s.bottom = "auto";
      } else {
        s.top = "50%";
        s.transform = "translateY(-50%)";
        s.bottom = "auto";
      }

      setDropdownStyle(s);
      requestAnimationFrame(() => {
        setMounted(true);
      });
    } else if (!open) {
      setMounted(false);
    }
  }, [open, options.length]);

  // 開いたら activeKey を現在の選択か先頭へ
  React.useEffect(() => {
    setActiveKey(open ? value || options[0]?.key || null : null);
  }, [open, value, options]);

  const handleSelect = (key: string) => {
    onChange(key);
    setOpen(false);
    setMounted(false);
    triggerRef.current?.focus();
  };

  const moveActive = (delta: 1 | -1) => {
    if (!options.length) return;
    const idx = options.findIndex((o) => o.key === activeKey);
    const nextIdx =
      idx < 0
        ? delta === 1
          ? 0
          : options.length - 1
        : (idx + delta + options.length) % options.length;
    setActiveKey(options[nextIdx].key);
  };

  // 印字可能文字を蓄積し、ラベルが前方一致する最初の option へ移動する。
  const handleTypeahead = (char: string) => {
    const ta = typeaheadRef.current;
    if (ta.timer !== null) window.clearTimeout(ta.timer);
    ta.buffer += char.toLowerCase();
    const match = options.find((o) => o.label.toLowerCase().startsWith(ta.buffer));
    if (match) setActiveKey(match.key);
    ta.timer = window.setTimeout(() => {
      ta.buffer = "";
      ta.timer = null;
    }, 600);
  };

  // アンマウント時にタイマを掃除する。
  React.useEffect(() => {
    const ta = typeaheadRef.current;
    return () => {
      if (ta.timer !== null) window.clearTimeout(ta.timer);
    };
  }, []);

  // フォーカスが wrapper の外へ抜けたら閉じる(Tab / blur 対応)。
  // relatedTarget が取れない環境向けに setTimeout + activeElement でも確認する。
  const handleBlur = (e: React.FocusEvent<HTMLDivElement>) => {
    const next = e.relatedTarget as Node | null;
    if (next && wrapperRef.current?.contains(next)) return;
    window.setTimeout(() => {
      const active = document.activeElement;
      if (wrapperRef.current && active && wrapperRef.current.contains(active)) return;
      setOpen(false);
      setMounted(false);
    }, 0);
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (disabled) return;
    const { key } = e;
    if (!open) {
      if (key === "Enter" || key === " " || key === "ArrowDown" || key === "ArrowUp") {
        e.preventDefault();
        setOpen(true);
      }
      return;
    }
    if (key === "Escape") {
      e.preventDefault();
      setOpen(false);
      setMounted(false);
      triggerRef.current?.focus();
    } else if (key === "ArrowDown") {
      e.preventDefault();
      moveActive(1);
    } else if (key === "ArrowUp") {
      e.preventDefault();
      moveActive(-1);
    } else if (key === "Home") {
      e.preventDefault();
      if (options[0]) setActiveKey(options[0].key);
    } else if (key === "End") {
      e.preventDefault();
      if (options.length) setActiveKey(options[options.length - 1].key);
    } else if (key === "Enter" || key === " ") {
      e.preventDefault();
      if (activeKey) handleSelect(activeKey);
    } else if (key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
      // 印字可能文字 1 字 → typeahead(Space は上の分岐で既に消費済み)。
      e.preventDefault();
      handleTypeahead(key);
    }
  };

  return (
    <div
      ref={wrapperRef}
      className={cn("relative inline-block min-w-[140px] select-none", disabled && "opacity-50")}
      onKeyDown={handleKeyDown}
      onBlur={handleBlur}
    >
      {/* 可視ラベル(任意)。combobox は aria-labelledby でこれを参照する */}
      {label != null && (
        <label id={labelId} className="mb-1 block text-xs font-bold text-[#725d42]">
          {label}
        </label>
      )}
      {/* フォーム送信用の隠し input(name 指定時のみ) */}
      {name != null && <input type="hidden" name={name} value={value} />}
      {/* トリガ:白面・薄クリーム枠・角丸 12px */}
      <div
        ref={triggerRef}
        className={cn(
          // min-h は現状の描画高(~40px)と同値の床。見た目は変えず、内容が縮んでも
          // タップしやすい高さを保証する(P2 touch target)。
          "flex min-h-10 cursor-pointer items-center justify-between rounded-xl border-2 border-[#e8dcc8] bg-white px-[13px] py-2 outline-none transition-all duration-200 focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-2",
          !disabled && "hover:border-[#d4c4a8] hover:bg-[#fffdf7]",
          disabled && "cursor-not-allowed bg-[#f5f5f0]",
        )}
        onClick={() => {
          if (!disabled) setOpen(!open);
        }}
        role="combobox"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listboxId : undefined}
        aria-activedescendant={open && activeKey ? optionId(activeKey) : undefined}
        aria-disabled={disabled || undefined}
        aria-label={ariaLabel}
        aria-labelledby={resolvedLabelledBy}
        tabIndex={disabled ? -1 : 0}
      >
        <span className={cn("text-sm", value ? "font-semibold text-[#725d42]" : "text-[#a09080]")}>
          {currentLabel}
        </span>
        <span
          className={cn(
            "flex items-center text-[#a09080] transition-transform duration-200",
            open && "rotate-180 text-[#19c8b9]",
          )}
          aria-hidden
        >
          <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
            <path
              d="M3 4.5L6 7.5L9 4.5"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        </span>
      </div>
      {open && mounted && (
        <div
          // dropdown:濃い黄面・角丸 28px・縦 12px パディング・フェードイン
          className="z-100 rounded-[28px] bg-[#FFEEA0] py-3 opacity-0 [animation:animal-fade-in_0.2s_ease_forwards]"
          style={dropdownStyle}
          role="listbox"
          id={listboxId}
          aria-label={ariaLabel}
          aria-labelledby={resolvedLabelledBy}
        >
          {options.map((option) => {
            const selected = value === option.key;
            // キーボード(activeKey)/マウス(hoveredKey)どちらの「現在項」も
            // 同じ見た目(カーソル + 太字)でハイライトする。
            const highlighted = (hoveredKey ?? activeKey) === option.key;
            return (
              <div
                key={option.key}
                id={optionId(option.key)}
                role="option"
                aria-selected={selected}
                className={cn(
                  "relative flex cursor-pointer items-center justify-center py-[10px] pr-[30px] pl-[14px] text-sm font-medium whitespace-nowrap text-[#725d42]",
                  (selected || highlighted) && "z-1 font-bold",
                )}
                onClick={() => {
                  handleSelect(option.key);
                }}
                onMouseEnter={() => {
                  setHoveredKey(option.key);
                  setActiveKey(option.key);
                }}
                onMouseLeave={() => setHoveredKey(null)}
              >
                {highlighted && (
                  // 原典の select-cursor.svg を左から滑り込ませる(自托管 /cursor)
                  <span
                    aria-hidden
                    className="pointer-events-none absolute top-1/2 left-[-12px] size-[35px] bg-[url(/cursor/select-cursor.svg)] bg-contain bg-center bg-no-repeat [animation:tbm-select-cursor-slide-in_0.5s_ease-out_forwards]"
                  />
                )}
                <span className="w-4 text-xs" aria-hidden />
                {option.label}
                {selected && (
                  // pillBar:選択項の下に敷く半透明の黄色バー
                  <div className="absolute inset-x-0 top-[56%] -z-1 mx-5 h-[14px] -translate-y-1/2 rounded-[7px] bg-[#FFCC00] opacity-30" />
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
