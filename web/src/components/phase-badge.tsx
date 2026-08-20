import { phaseLabel } from "@/lib/services";

// バッジの色語彙(単一真源)。同じ色を別の意味に使い回すと画面が嘘をつく — 例えば
// 「未反映(要デプロイ)」の琥珀を停止中に流用すると、停止中の service が未反映に見える。
const TONE = {
  success: "bg-[#2f6b3f]/15 text-[#3f8a55]",
  warn: "bg-[#b5862a]/15 text-[#b5862a]",
  danger: "bg-[#e05a5a]/15 text-[#e05a5a]",
  muted: "bg-muted text-muted-foreground",
} as const;

// 小さな状態バッジ。size="sm" は行内(一覧の 1 行)向けの詰めた寸法。
export function Badge({
  tone,
  size = "md",
  children,
}: {
  tone: keyof typeof TONE;
  size?: "sm" | "md";
  children: React.ReactNode;
}) {
  const pad = size === "sm" ? "px-2 py-0.5" : "px-2.5 py-1";
  return (
    <span className={`shrink-0 rounded-full ${pad} text-xs font-bold ${TONE[tone]}`}>
      {children}
    </span>
  );
}

// service の phase バッジ(一覧 + 詳細ページで共用)。色は観測された段階で決まる。
// running=緑 / deploying=琥珀 / failed=赤 / その他(created・stopped)=灰。
// 色分けは wire 値(英語 enum)で判定し、表示は日本語ラベル(phaseLabel)。
export function PhaseBadge({ phase }: { phase: string }) {
  const tone =
    phase === "running"
      ? "success"
      : phase === "deploying"
        ? "warn"
        : phase === "failed"
          ? "danger"
          : "muted";
  return <Badge tone={tone}>{phaseLabel(phase)}</Badge>;
}
