import * as React from "react";

// animal-island-ui(guokaigdg)の Typewriter を移植。純ロジックのみ:
// CSS・className・外層ラッパを一切持たず、布局/字号/色/フォントに零影響。
// children を逐字表示し、元の要素構造/改行/様式を保つ。

export interface TypewriterProps {
  /** 逐字表示する内容。ReactNode 対応で元の要素構造/改行/様式を保つ */
  children?: React.ReactNode;
  /** 1 字あたりの間隔 (ms)、既定 90 */
  speed?: number;
  /**
   * 外部からの再生トリガ。値が変わるたびにアニメを再起動する。
   * よくある使い方は弾窗の open 回数や増加する key を渡すこと。
   */
  trigger?: unknown;
  /** 自動で頭から再生するか。既定 true。false で全文を即表示 */
  autoPlay?: boolean;
  /** 再生完了コールバック */
  onDone?: () => void;
}

/**
 * ReactNode 内の純テキスト総長を再帰的に数える(打字機の進度を駆動)
 */
const countText = (node: React.ReactNode): number => {
  if (node == null || typeof node === "boolean") return 0;
  if (typeof node === "string" || typeof node === "number") return String(node).length;
  if (Array.isArray(node)) return node.reduce<number>((s, n) => s + countText(n), 0);
  if (React.isValidElement(node)) {
    return countText((node.props as { children?: React.ReactNode }).children);
  }
  return 0;
};

/**
 * prefers-reduced-motion: reduce を購読する。SSR(window 無し)では false を返す。
 * reduce のときは打字機アニメを丸ごと飛ばし全文を即時表示するために使う。
 */
const usePrefersReducedMotion = (): boolean => {
  const getInitial = () =>
    typeof window !== "undefined" && typeof window.matchMedia === "function"
      ? window.matchMedia("(prefers-reduced-motion: reduce)").matches
      : false;

  const [reduced, setReduced] = React.useState(getInitial);

  React.useEffect(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") return;
    const mql = window.matchMedia("(prefers-reduced-motion: reduce)");
    const onChange = () => setReduced(mql.matches);
    onChange();
    mql.addEventListener("change", onChange);
    return () => mql.removeEventListener("change", onChange);
  }, []);

  return reduced;
};

interface RenderState {
  remaining: number;
  stopped: boolean;
}

/**
 * 残り可表示字数で ReactNode を裁断し、元の要素構造/改行/様式を保つ。
 */
const renderTruncated = (
  node: React.ReactNode,
  state: RenderState,
  keyPrefix = "tw",
): React.ReactNode => {
  if (state.stopped) return null;
  if (node == null || typeof node === "boolean") return null;

  if (typeof node === "string" || typeof node === "number") {
    const text = String(node);
    if (state.remaining >= text.length) {
      state.remaining -= text.length;
      return text;
    }
    const shown = text.slice(0, state.remaining);
    state.remaining = 0;
    state.stopped = true;
    return shown;
  }

  if (Array.isArray(node)) {
    return node.map((child, i) => (
      <React.Fragment key={`${keyPrefix}-${i}`}>
        {renderTruncated(child, state, `${keyPrefix}-${i}`)}
      </React.Fragment>
    ));
  }

  if (React.isValidElement(node)) {
    const props = node.props as { children?: React.ReactNode };
    const childContent = renderTruncated(props.children, state, keyPrefix);
    return React.cloneElement(node, undefined, childContent);
  }

  return null;
};

/**
 * Typewriter 打字機コンポーネント
 * - 字を 1 つずつ表示し、元 children の要素構造/改行/様式を保つ
 * - 外層ラッパを一切導入せず、布局/字号/色/フォントに零影響
 */
export const Typewriter: React.FC<TypewriterProps> = ({
  children,
  speed = 90,
  trigger,
  autoPlay = true,
  onDone,
}) => {
  const prefersReducedMotion = usePrefersReducedMotion();
  const total = React.useMemo(() => countText(children), [children]);
  // reduced-motion 時は autoPlay によらず全文を初期表示(逐字アニメを飛ばす)。
  const [count, setCount] = React.useState(autoPlay && !prefersReducedMotion ? 0 : total);
  const timerRef = React.useRef<number | null>(null);

  React.useEffect(() => {
    if (timerRef.current) window.clearInterval(timerRef.current);
    // reduced-motion が有効なら、API はそのままに逐字アニメだけ無効化し全文を即表示。
    if (!autoPlay || prefersReducedMotion) {
      setCount(total);
      return;
    }
    setCount(0);
    if (total === 0) return;
    timerRef.current = window.setInterval(() => {
      setCount((c) => {
        if (c >= total) {
          if (timerRef.current) window.clearInterval(timerRef.current);
          return c;
        }
        return c + 1;
      });
    }, speed);
    return () => {
      if (timerRef.current) window.clearInterval(timerRef.current);
    };
  }, [total, speed, trigger, autoPlay, prefersReducedMotion]);

  React.useEffect(() => {
    if (total > 0 && count >= total) onDone?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [count, total]);

  const state: RenderState = { remaining: count, stopped: false };
  return <>{renderTruncated(children, state)}</>;
};

Typewriter.displayName = "Typewriter";
