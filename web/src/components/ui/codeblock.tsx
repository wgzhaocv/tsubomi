import * as React from "react";
import { Check, Copy } from "lucide-react";

import { cn } from "@/lib/utils";

// animal-island-ui(guokaigdg)の CodeBlock を移植。原典は <pre> にインライン
// スタイルで温かみのあるダークパレットを当て、自前の正規表現で JSX/TS を
// シンタックスハイライトする(ライブラリ非依存)。色・寸法は原典の COLORS /
// codeBlockStyle を厳密に踏襲し、Tailwind v4 の任意値で literal hex/px として
// 保持する。tsubomi 拡張:language / title / showCopy(コピーボタン)を追加。
// 原典の code / style / className の prop API はそのまま維持する。

// 原典 COLORS をそのまま literal で保持(トークン種別 → 文字色)。
const TOKEN_COLOR = {
  comment: "#6b5e50",
  string: "#a8d4a0",
  keyword: "#d4a0e0",
  react: "#e06c75",
  component: "#80c0e0",
  func: "#61afef",
  prop: "#e8c87a",
  jsx: "#f0a870",
  operator: "#d4b896",
  number: "#a8d4a0",
  default: "#e8d5bc",
} as const;

type TokenColor = (typeof TOKEN_COLOR)[keyof typeof TOKEN_COLOR];

interface Token {
  start: number;
  end: number;
  color: TokenColor;
}

// 原典 highlightJSX の移植:重なり順を尊重しながらトークンを span に切り出す。
function highlightJSX(code: string): React.ReactNode[] {
  const tokens: Token[] = [];

  const addPattern = (regex: RegExp, color: TokenColor) => {
    const re = new RegExp(
      regex.source,
      regex.flags.includes("g") ? regex.flags : regex.flags + "g",
    );
    let match: RegExpExecArray | null;
    while ((match = re.exec(code)) !== null) {
      tokens.push({
        start: match.index,
        end: match.index + match[0].length,
        color,
      });
    }
  };

  addPattern(/\/\*[\s\S]*?\*\//g, TOKEN_COLOR.comment);
  addPattern(/\/\/.*$/gm, TOKEN_COLOR.comment);
  addPattern(/`[^`]*`/g, TOKEN_COLOR.string);
  addPattern(/"[^"]*"/g, TOKEN_COLOR.string);
  addPattern(/'[^']*'/g, TOKEN_COLOR.string);
  addPattern(/<\/?[A-Z][\w.$]*/g, TOKEN_COLOR.jsx);
  addPattern(/<\/?[a-z][\w-]*/g, TOKEN_COLOR.jsx);
  addPattern(/\/?>/g, TOKEN_COLOR.jsx);
  addPattern(
    /\b(React|useState|useEffect|useCallback|useMemo|useRef|useContext|useReducer|useLayoutEffect|useImperativeHandle|useDebugValue|createContext|createElement|cloneElement|Fragment|Suspense|lazy|memo|forwardRef|useId|FC|ReactNode|ReactElement|CSSProperties)\b/g,
    TOKEN_COLOR.react,
  );
  addPattern(/\b(true|false)\b/g, TOKEN_COLOR.keyword);
  addPattern(/\b(null|undefined|void|NaN|Infinity)\b/gi, TOKEN_COLOR.keyword);
  addPattern(/\b\d+\.?\d*\b/g, TOKEN_COLOR.number);
  addPattern(
    /\b(import|from|as|export|default|const|let|var|function|return|if|else|for|while|switch|case|break|continue|try|catch|throw|finally|new|typeof|instanceof|async|await|type|interface)\b/gi,
    TOKEN_COLOR.keyword,
  );
  addPattern(/\b[A-Z][a-zA-Z0-9_$]*\b/g, TOKEN_COLOR.component);
  addPattern(/\b[a-z][a-zA-Z0-9_$]*\s*(?=\()/g, TOKEN_COLOR.func);
  addPattern(/\b[a-zA-Z_$][\w$]*\s*(?==)/g, TOKEN_COLOR.prop);
  addPattern(/>|===|!==|==|!=|<=|>=|&&|\|\||[+\-*/%=<>!&|^~?:]/g, TOKEN_COLOR.operator);
  addPattern(/[{}[\]();,]/g, TOKEN_COLOR.operator);

  tokens.sort((a, b) => a.start - b.start);

  const result: React.ReactNode[] = [];
  let pos = 0;

  for (const token of tokens) {
    if (token.start < pos) continue;

    if (token.start > pos) {
      result.push(
        <span key={`t${pos}`} style={{ color: TOKEN_COLOR.default }}>
          {code.slice(pos, token.start)}
        </span>,
      );
    }

    result.push(
      <span key={`s${token.start}`} style={{ color: token.color }}>
        {code.slice(token.start, token.end)}
      </span>,
    );
    pos = token.end;
  }

  if (pos < code.length) {
    result.push(
      <span key={`e${pos}`} style={{ color: TOKEN_COLOR.default }}>
        {code.slice(pos)}
      </span>,
    );
  }

  return result;
}

// シンタックスハイライトを当てない言語(ログや素のテキスト)。JSX 用の正規表現で
// 着色すると意味のない色が付く上、長文 + 自動更新だと描画毎に全文へ ~16 趟の正規表現を
// 走らせて無駄。これらは生文字列をそのまま <pre> へ流す(色は既定の #e8d5bc 一色)。
const PLAINTEXT_LANGS = new Set(["log", "text", "txt", "plain"]);

function isPlaintext(language?: string): boolean {
  return language != null && PLAINTEXT_LANGS.has(language.toLowerCase());
}

export interface CodeBlockProps extends Omit<React.HTMLAttributes<HTMLDivElement>, "title"> {
  /** ハイライト表示するソースコード */
  code: string;
  /** 言語ラベル(ヘッダ右側に小さく表示。コピー挙動には影響しない) */
  language?: string;
  /** ヘッダ左側に出すタイトル(ファイル名など) */
  title?: React.ReactNode;
  /** コピーボタンを表示(既定 true) */
  showCopy?: boolean;
  /** <pre> へ直接渡す追加スタイル(原典 API) */
  preStyle?: React.CSSProperties;
}

// 等幅フォントスタック(原典 codeBlockStyle の fontFamily を踏襲)。
const MONO = "font-['SF_Mono','Fira_Code','Cascadia_Code',Consolas,monospace]";

export function CodeBlock({
  code,
  language,
  title,
  showCopy = true,
  preStyle,
  className,
  ...rest
}: CodeBlockProps) {
  const [copied, setCopied] = React.useState(false);
  const timerRef = React.useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  React.useEffect(() => {
    return () => {
      if (timerRef.current !== undefined) clearTimeout(timerRef.current);
    };
  }, []);

  const handleCopy = React.useCallback(async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      if (timerRef.current !== undefined) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => setCopied(false), 1600);
    } catch {
      setCopied(false);
    }
  }, [code]);

  const hasHeader = title != null || language != null || showCopy;

  return (
    <div
      className={cn(
        // 容器:原典 codeBlockStyle の bg/border/radius を literal で保持。
        "relative overflow-hidden rounded-[20px] border border-[#3d3028] bg-[#2b2118]",
        className,
      )}
      {...rest}
    >
      {hasHeader && (
        <div className="flex items-center justify-between gap-3 border-b border-[#3d3028] px-6 py-2.5">
          <div className="flex min-w-0 items-center gap-2">
            {title != null && (
              <span className="truncate text-[13px] font-semibold text-[#e8d5bc]">{title}</span>
            )}
            {language != null && (
              <span
                className={cn(
                  "shrink-0 text-[11px] font-semibold tracking-[0.04em] text-[#6b5e50] uppercase",
                  MONO,
                )}
              >
                {language}
              </span>
            )}
          </div>
          {showCopy && (
            <button
              type="button"
              onClick={handleCopy}
              aria-label={copied ? "コピーしました" : "コードをコピー"}
              className="inline-flex shrink-0 items-center gap-1.5 rounded-[10px] border border-[#3d3028] bg-[#3d3028] px-2.5 py-1 text-[12px] font-semibold text-[#e8d5bc] outline-none transition-colors duration-150 hover:bg-[#4a3a2e] focus-visible:[outline:2px_solid_#f0a870] focus-visible:outline-offset-2"
            >
              {copied ? (
                <Check className="size-3.5 text-[#a8d4a0]" />
              ) : (
                <Copy className="size-3.5" />
              )}
              {copied ? "コピー済み" : "コピー"}
            </button>
          )}
        </div>
      )}
      <pre
        className={cn(
          // 原典 codeBlockStyle:padding 20px 24px / fontSize 14 / lineHeight 1.7
          // / fontWeight 600 / color #e8d5bc / whiteSpace pre / overflow auto。
          "m-0 overflow-auto px-6 py-5 text-[14px] leading-[1.7] font-semibold whitespace-pre text-[#e8d5bc] tab-4",
          MONO,
        )}
        style={preStyle}
      >
        {/* a11y(P2 セマンティクス):トークンを <code> でラップし pre > code に。
            見た目は <pre> から継承させるため、ブラウザ既定の等幅/色を持ち込まない
            よう font/color を inherit に倒すだけ(色・寸法は一切変えない)。 */}
        <code className="font-[inherit] text-inherit">
          {isPlaintext(language) ? code : highlightJSX(code)}
        </code>
      </pre>
      {/* a11y(P2 コピーのライブ通知):コピー成功をスクリーンリーダーへ通知する
          視覚非表示の status 領域。視覚レイアウトには影響しない(sr-only 相当)。 */}
      <span
        role="status"
        aria-live="polite"
        className="absolute -m-px h-px w-px overflow-hidden border-0 p-0 whitespace-nowrap [clip:rect(0,0,0,0)] [clip-path:inset(50%)]"
      >
        {copied ? "コピーしました" : ""}
      </span>
    </div>
  );
}
