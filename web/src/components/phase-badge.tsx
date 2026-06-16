import { phaseLabel } from "@/lib/services";

// service の phase バッジ(一覧 + 詳細ページで共用)。色は観測された段階で決まる。
// running=緑 / deploying=琥珀 / failed=赤 / その他(created・stopped)=灰。
// 色分けは wire 値(英語 enum)で判定し、表示は日本語ラベル(phaseLabel)。
export function PhaseBadge({ phase }: { phase: string }) {
  const tone =
    phase === "running"
      ? "bg-[#2f6b3f]/15 text-[#3f8a55]"
      : phase === "deploying"
        ? "bg-[#b5862a]/15 text-[#b5862a]"
        : phase === "failed"
          ? "bg-[#e05a5a]/15 text-[#e05a5a]"
          : "bg-muted text-muted-foreground";
  return (
    <span className={`shrink-0 rounded-full px-2.5 py-1 text-xs font-bold ${tone}`}>
      {phaseLabel(phase)}
    </span>
  );
}
