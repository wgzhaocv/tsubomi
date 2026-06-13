import * as React from "react";
import { createPortal } from "react-dom";

import { Button } from "@/components/ui/button";
import { Typewriter } from "@/components/ui/typewriter";
import { cn } from "@/lib/utils";

// animal-island-ui(guokaigdg)の Modal を移植。原典の clip-path 有機ブロブ形・
// 暖クリーム面・打字機本文・フォーカストラップを厳密に踏襲。色・寸法は
// src/components/Modal/modal.module.less を踏襲。tsubomi の差分:
// <Cursor> ラッパは移植しないので外す;footer 既定は our <Button> 2 つ
// (キャンセル / OK);フォントは reference の 'animal-dialog' を捨て、
// プロジェクトの --font-sans を継承する。

// フォーカス可能要素のセレクタ(フォーカストラップ用)
const FOCUSABLE_SELECTOR = [
  "a[href]",
  "area[href]",
  "button:not([disabled])",
  'input:not([disabled]):not([type="hidden"])',
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
  "audio[controls]",
  "video[controls]",
  '[contenteditable]:not([contenteditable="false"])',
].join(",");

const getFocusable = (root: HTMLElement): HTMLElement[] => {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
    (el) => !el.hasAttribute("disabled") && el.getAttribute("aria-hidden") !== "true",
  );
};

// インライン SVG clip-path — Dialog と同じ有機ブロブ形(path は原典逐字)
const ClipDef: React.FC = () => (
  <svg style={{ position: "absolute", width: 0, height: 0 }} aria-hidden>
    <clipPath id="animal-modal-clip" clipPathUnits="objectBoundingBox">
      <path d="M0.501,0.005 L0.501,0.005 L0.523,0.005 L0.549,0.006 C0.704,0.01,0.796,0.017,0.825,0.027 L0.827,0.028 C0.872,0.045,0.939,0.044,0.978,0.17 C1,0.254,1,0.365,0.99,0.505 L0.988,0.513 C0.979,0.558,0.971,0.598,0.965,0.633 C0.956,0.689,0.979,0.77,0.964,0.865 C0.953,0.928,0.921,0.966,0.869,0.979 C0.821,0.986,0.773,0.992,0.726,0.995 L0.712,0.996 L0.694,0.997 C0.648,1,0.586,1,0.507,1 L0.501,1 L0.464,1 C0.385,1,0.325,0.998,0.283,0.995 C0.234,0.992,0.184,0.987,0.133,0.979 C0.081,0.966,0.05,0.928,0.039,0.865 C0.023,0.77,0.047,0.689,0.037,0.633 C0.031,0.595,0.023,0.552,0.013,0.505 C-0.006,0.365,-0.002,0.254,0.024,0.17 C0.064,0.045,0.13,0.045,0.174,0.028 L0.175,0.028 C0.204,0.017,0.303,0.009,0.474,0.005 L0.501,0.005" />
    </clipPath>
  </svg>
);

export interface ModalProps {
  /** 表示するか */
  open: boolean;
  /** タイトル */
  title?: React.ReactNode;
  /** 幅 */
  width?: number | string;
  /** 遮罩クリックで閉じる */
  maskClosable?: boolean;
  /** フッターボタン領域。undefined=既定 / null=非表示 */
  footer?: React.ReactNode | null;
  /** 閉じるコールバック */
  onClose?: () => void;
  /** 確認コールバック */
  onOk?: () => void;
  /** カスタム内容 */
  children?: React.ReactNode;
  className?: string;
  /** 打字機の 1 字あたり間隔 (ms)、既定 80 */
  typeSpeed?: number;
  /** 打字機効果を有効にするか、既定 true */
  typewriter?: boolean;
  /** 遮罩層のカスタム様式 */
  maskStyle?: React.CSSProperties;
  /**
   * 対話框のアクセシブル名(直接文字列指定)。`title` も `aria-labelledby` も
   * 無いときの名前付け手段。a11y: dialog には必ず名前が要る。
   */
  "aria-label"?: string;
  /**
   * 対話框のアクセシブル名(他要素の id を参照)。`title` がある場合は
   * 内部の titleId が優先されるため無視される。
   */
  "aria-labelledby"?: string;
  /**
   * 補足説明として参照させる要素の id(直接指定)。これを渡すと本文 id への
   * 自動 aria-describedby より優先される。
   */
  "aria-describedby"?: string;
  /**
   * 本文を aria-describedby として参照させるか、既定 true。本文が長文・複雑で
   * 読み上げが冗長になる場合は false にして opt-out できる。明示的な
   * `aria-describedby` が渡された場合はそちらが常に優先される。
   */
  describedBy?: boolean;
}

export const Modal: React.FC<ModalProps> = ({
  open,
  title,
  width = 520,
  maskClosable = true,
  footer,
  onClose,
  onOk,
  children,
  className,
  typeSpeed = 80,
  typewriter = true,
  maskStyle,
  "aria-label": ariaLabel,
  "aria-labelledby": ariaLabelledby,
  "aria-describedby": ariaDescribedby,
  describedBy = true,
}) => {
  // open が true になるたびに打字機を再起動する
  const [playKey, setPlayKey] = React.useState(0);
  React.useEffect(() => {
    if (open) setPlayKey((k) => k + 1);
  }, [open]);

  const dialogRef = React.useRef<HTMLDivElement>(null);
  const previouslyFocusedRef = React.useRef<HTMLElement | null>(null);

  // 開いたとき発火元を記録 + フォーカスを対話框へ送る;閉じたとき返す
  React.useEffect(() => {
    if (!open) return;
    previouslyFocusedRef.current = (document.activeElement as HTMLElement) ?? null;
    // 次の microtask を待ち、対話框ノードがマウント・createPortal 完了した後にする
    const id = window.setTimeout(() => {
      const dialog = dialogRef.current;
      if (!dialog) return;
      const focusables = getFocusable(dialog);
      (focusables[0] ?? dialog).focus();
    }, 0);
    return () => {
      window.clearTimeout(id);
      previouslyFocusedRef.current?.focus?.();
    };
  }, [open]);

  // ESC で閉じる + Tab/Shift+Tab フォーカストラップ
  React.useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose?.();
        return;
      }
      if (e.key !== "Tab") return;
      const dialog = dialogRef.current;
      if (!dialog) return;
      const focusables = getFocusable(dialog);
      if (focusables.length === 0) {
        e.preventDefault();
        dialog.focus();
        return;
      }
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      const active = document.activeElement as HTMLElement | null;
      if (e.shiftKey) {
        if (active === first || !dialog.contains(active)) {
          e.preventDefault();
          last.focus();
        }
      } else {
        if (active === last || !dialog.contains(active)) {
          e.preventDefault();
          first.focus();
        }
      }
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [open, onClose]);

  // スクロールロック。スクロールバー消失で「内容」が右に広がってズレるのを防ぐため、
  // その幅だけ body に padding-right を補う。背景は固定の全画面レイヤー(body::before、
  // 100vw)なので body の padding に影響されずズレない。暗幕は fixed inset-0 で全幅を覆う。
  React.useEffect(() => {
    if (!open) return;
    const scrollbarW = window.innerWidth - document.documentElement.clientWidth;
    const prevOverflow = document.body.style.overflow;
    const prevPadding = document.body.style.paddingRight;
    document.body.style.overflow = "hidden";
    if (scrollbarW > 0) {
      document.body.style.paddingRight = `${scrollbarW}px`;
    }
    return () => {
      document.body.style.overflow = prevOverflow;
      document.body.style.paddingRight = prevPadding;
    };
  }, [open]);

  // 背景を inert にする。対話框は createPortal で document.body 直下(#root の外)に
  // 出るので、#root を inert + aria-hidden にすれば支援技術が背景へ到達できなくなる。
  // SSR / #root 不在を考慮して null ガードし、閉じたら元の状態へ復元する。
  React.useEffect(() => {
    if (!open) return;
    if (typeof document === "undefined") return;
    const root = document.getElementById("root");
    if (!root) return;
    const hadInert = root.hasAttribute("inert");
    const prevAriaHidden = root.getAttribute("aria-hidden");
    root.setAttribute("inert", "");
    root.setAttribute("aria-hidden", "true");
    return () => {
      // 開いた時に既に inert だった場合は外さない(他の重なり対話框を尊重)
      if (!hadInert) root.removeAttribute("inert");
      if (prevAriaHidden === null) {
        root.removeAttribute("aria-hidden");
      } else {
        root.setAttribute("aria-hidden", prevAriaHidden);
      }
    };
  }, [open]);

  const handleMaskClick = React.useCallback(() => {
    if (maskClosable) onClose?.();
  }, [maskClosable, onClose]);

  const handleContentClick = React.useCallback((e: React.MouseEvent) => {
    e.stopPropagation();
  }, []);

  const idPrefix = `animal-modal-${React.useId().replace(/:/g, "")}`;
  const titleId = `${idPrefix}-title`;
  const bodyId = `${idPrefix}-body`;

  // アクセシブル名の解決:title があれば内部の titleId を最優先、無ければ
  // 呼び出し側の aria-labelledby、それも無ければ aria-label。dialog は必ず
  // 名前を持つべきだが、どれも無い場合は undefined のまま(呼び出し側の責務)。
  const resolvedLabelledby = title ? titleId : ariaLabelledby;
  const resolvedLabel = title || resolvedLabelledby ? undefined : ariaLabel;

  // 補足説明の解決:明示的な aria-describedby を最優先。無ければ describedBy が
  // true のとき本文 id を指す(既定);false なら opt-out して undefined。
  const resolvedDescribedby = ariaDescribedby ?? (describedBy ? bodyId : undefined);

  if (!open) return null;

  // 既定フッター:our <Button> 2 つ(キャンセル / OK)
  const defaultFooter = (
    <>
      <Button type="primary" onClick={onClose}>
        キャンセル
      </Button>
      <Button type="primary" onClick={onOk}>
        OK
      </Button>
    </>
  );

  const modalContent = (
    // 遮罩層:fixed・中央寄せ・半透明黒 + フェードイン
    <div
      className="fixed inset-0 z-1000 flex items-center justify-center bg-[rgba(0,0,0,0.35)] animate-[animal-fade-in_0.25s_ease]"
      style={maskStyle}
      onClick={handleMaskClick}
    >
      <div
        ref={dialogRef}
        className={cn(
          "relative flex max-h-[calc(100vh-64px)] max-w-[calc(100vw-32px)] flex-col animate-[animal-zoom-in_0.3s_ease]",
          className,
        )}
        style={{ width }}
        onClick={handleContentClick}
        role="dialog"
        aria-modal="true"
        aria-labelledby={resolvedLabelledby}
        aria-label={resolvedLabel}
        aria-describedby={resolvedDescribedby}
        tabIndex={-1}
      >
        <ClipDef />
        {/* clip-path で有機ブロブ形に切り抜く本体 */}
        <div className="flex flex-col overflow-hidden bg-[rgb(247,243,223)] p-[48px_48px_32px] text-[rgb(128,115,89)] [clip-path:url(#animal-modal-clip)]">
          {title && (
            <div className="flex items-center justify-between pb-3.75">
              <div className="text-[28px] font-bold text-[rgba(114,93,66,1)]" id={titleId}>
                {title}
              </div>
            </div>
          )}
          <div
            // overflow-y-auto は overflow-x も auto に格上げされ横方向もクリップする。
            // 中の要素の focus 枠(outline は box の外側に出る)が左右端で切れるので、
            // px で枠の逃げ場を作り、同量の負マージンで見た目の位置を元へ戻す。
            className="-mx-1.5 flex flex-1 flex-col items-start overflow-y-auto px-1.5 pb-5 text-[20px] font-semibold leading-[1.6] text-[#8a7b66]"
            id={bodyId}
          >
            {typewriter ? (
              <Typewriter speed={typeSpeed} trigger={playKey}>
                {children}
              </Typewriter>
            ) : (
              children
            )}
          </div>
          {footer !== null && (
            <div className="flex items-center justify-end gap-3">
              {footer === undefined ? defaultFooter : footer}
            </div>
          )}
        </div>
      </div>
    </div>
  );

  return createPortal(modalContent, document.body);
};

Modal.displayName = "Modal";
