import * as React from "react";
import { Globe } from "lucide-react";

import { Typewriter } from "@/components/ui/typewriter";
import { cn } from "@/lib/utils";

// Claude Code を模した端末パネル。視口に入って「少し経ってから」script を上から 1 行ずつ
// 打字機で再生し、AI が作ってデプロイ → 公開 URL を返す様子をデモする。中身はすべて **例**
// (実在サイトに見せない:URL は <a> にせず「例」バッジを付け、ヘッダに「デモ」を出す)。

export type SessionRole = "user" | "claude" | "url";
export interface SessionLine {
  role: SessionRole;
  text?: string;
}

const MONO = "font-['SF_Mono','Fira_Code','Cascadia_Code',Consolas,monospace]";
const DELAY_MS = 250; // 触発線に達してから打ち始めるまでの「ひと呼吸」。

// 要素が視口に入ったら一度だけ true を返す(IntersectionObserver)。
function useInView<T extends Element>() {
  const ref = React.useRef<T>(null);
  const [inView, setInView] = React.useState(false);
  React.useEffect(() => {
    const el = ref.current;
    if (!el || inView) return;
    if (typeof IntersectionObserver === "undefined") {
      setInView(true);
      return;
    }
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          setInView(true);
          io.disconnect();
        }
      },
      // 画面の「中央やや下」に来たら発火する:rootMargin で実効ビューポートの下端を 40%
      // 引き上げ(= 画面の上から 60% の線)、その線をパネル上端が越えた瞬間に起こす。
      { threshold: 0, rootMargin: "0px 0px -40% 0px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [inView]);
  return { ref, inView };
}

export function ClaudeSession({ script }: { script: SessionLine[] }) {
  const { ref, inView } = useInView<HTMLDivElement>();
  const [started, setStarted] = React.useState(false);
  const [shown, setShown] = React.useState(0); // 打ち終えた行数 = いま打っている行の index
  const advance = React.useCallback(() => setShown((s) => s + 1), []);

  React.useEffect(() => {
    if (!inView || started) return;
    const t = window.setTimeout(() => setStarted(true), DELAY_MS);
    return () => window.clearTimeout(t);
  }, [inView, started]);

  const visible = started ? script.slice(0, shown + 1) : [];

  return (
    <div
      ref={ref}
      className="overflow-hidden rounded-2xl border border-[#3d3028] bg-[#2b2118] shadow-[0_4px_0_0_#1c150f]"
    >
      {/* 窓のタイトル = 第 3 ステップで開いた my-app フォルダ(端末は cwd をここに出す)。
          これで「そのフォルダの中で Claude Code が動いている」ことが伝わる。 */}
      <div className="flex items-center gap-2 border-b border-[#3d3028] px-4 py-2.5">
        <span className="size-2.5 rounded-full bg-[#e87878]" />
        <span className="size-2.5 rounded-full bg-[#f5c31c]" />
        <span className="size-2.5 rounded-full bg-[#7cc47c]" />
        <span className={cn("ml-1.5 truncate text-[12px] font-semibold text-[#cdbb9e]", MONO)}>
          📁 ~/my-app
        </span>
        <span className="ml-auto shrink-0 text-[12px] font-bold text-[#e8d5bc]">Claude Code</span>
        <span className="shrink-0 rounded bg-[#3d3028] px-1.5 py-0.5 text-[10px] font-bold tracking-wide text-[#bda98c]">
          デモ
        </span>
      </div>
      <div
        className={cn("flex min-h-28 flex-col gap-2 px-4 py-4 text-[13px] leading-relaxed", MONO)}
      >
        {!started && <span className="text-[#6b5e50]">▌</span>}
        {visible.map((ln, i) => {
          const typing = started && i === shown && shown < script.length;
          return <Line key={i} line={ln} typing={typing} onDone={advance} />;
        })}
      </div>
    </div>
  );
}

function Line({
  line,
  typing,
  onDone,
}: {
  line: SessionLine;
  typing: boolean;
  onDone: () => void;
}) {
  if (line.role === "url") return <UrlLine typing={typing} onDone={onDone} />;
  const body = (
    <>
      {typing ? <Typewriter onDone={onDone}>{line.text}</Typewriter> : line.text}
      {typing && <span className="ml-0.5 animate-pulse">▌</span>}
    </>
  );
  // 利用者の発話は **右側の強調された吹き出し**(対話感)、Claude は左側に ⏺ 行で。
  if (line.role === "user") {
    return (
      <div className="flex justify-end">
        <div className="max-w-[85%] rounded-2xl rounded-br-sm border border-[#0CC0B5]/40 bg-[rgba(12,192,181,0.18)] px-3 py-1.5 font-bold text-[#dcf2ed]">
          {body}
        </div>
      </div>
    );
  }
  return (
    <div className="flex max-w-[90%] gap-2">
      <span className="shrink-0 font-bold text-[#f0a870]">⏺</span>
      <span className="text-[#cdbb9e]">{body}</span>
    </div>
  );
}

// 公開 URL 行(例)。打ち始めの行になったら淡入で出し、ひと呼吸おいて完了扱いにする。
function UrlLine({ typing, onDone }: { typing: boolean; onDone: () => void }) {
  React.useEffect(() => {
    if (!typing) return;
    const t = window.setTimeout(onDone, 300);
    return () => window.clearTimeout(t);
  }, [typing, onDone]);
  // ドメインは実際に開いているデプロイ先から取る(ハードコードしない)。サービスは
  // 平台ドメインの一級子域(例:<名前>.<このサイトのドメイン>)。
  return (
    <div className="flex animate-[animal-fade-in_0.4s_ease_both] items-center gap-2 pl-5">
      <Globe className="size-4 shrink-0 text-[#7cc47c]" />
      <span className="font-bold text-[#a8d4a0]">https://my-app.{window.location.host}</span>
      <span className="rounded bg-[#3d3028] px-1.5 py-0.5 text-[10px] font-bold text-[#bda98c]">
        例
      </span>
    </div>
  );
}
