import * as React from "react";

import { cn } from "@/lib/utils";

// shadcn の Skeleton を本デザイン(どうぶつの森風・暖クリーム)に寄せたもの。
// 既定の bg-muted ではなく、カード罫線と同系の温かいタン(rgb(196,184,158) ≈ #c4b89e。
// 用量バーの下地と揃える)を淡く敷き、角丸は本デザイン語彙に合わせて丸める(角は立てない)。
// API は shadcn 同様 className を重ねるだけ — 高さ / 幅は呼び出し側が指定する。
function Skeleton({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      className={cn("animate-pulse rounded-lg bg-[rgba(196,184,158,0.45)]", className)}
      {...props}
    />
  );
}

export { Skeleton };
