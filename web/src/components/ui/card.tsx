import * as React from "react";

import { cn } from "@/lib/utils";

// どうぶつの森風カード。原典 animal-island-ui の Card に準拠:
// 丸み 20px・暖クリーム面(rgb(247,243,223))・本文 500・枠なし・影なし。
// hover の持ち上げ(translateY(-2px))は interactive=クリック可能なカードのみ
// 付与する(原典は既定で hover するが、本プロジェクトでは静的パネルは動かさない)。
// a11y: interactive のときは div でもキーボード操作できるよう role="button" /
// tabIndex=0 / Enter・Space で click を発火させる(原典の hover は維持)。
// <button> へ強制描画はしない(カードは任意の内容を持つため)。利用側が
// role / tabIndex / onKeyDown を渡した場合はそれを優先する。
function Card({
  className,
  interactive = false,
  role,
  tabIndex,
  onKeyDown,
  ...props
}: React.ComponentProps<"div"> & { interactive?: boolean }) {
  // Enter / Space でクリックを発火(利用側 onClick があればそれも呼ばれる)。
  const handleKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    onKeyDown?.(event);
    if (!interactive || event.defaultPrevented) return;
    if (event.key === "Enter" || event.key === " ") {
      // Space のページスクロール抑止。要素の click() で props.onClick も発火する。
      event.preventDefault();
      event.currentTarget.click();
    }
  };

  return (
    <div
      data-slot="card"
      data-interactive={interactive || undefined}
      // 利用側が明示した値を優先しつつ、interactive の既定値を補う。
      role={role ?? (interactive ? "button" : undefined)}
      tabIndex={tabIndex ?? (interactive ? 0 : undefined)}
      onKeyDown={interactive ? handleKeyDown : onKeyDown}
      className={cn(
        "flex flex-col gap-6 rounded-[20px] bg-card py-4 font-medium text-card-foreground",
        interactive &&
          "cursor-pointer outline-none transition-transform duration-300 ease-[ease] hover:-translate-y-0.5 focus-visible:[outline:2px_solid_#19c8b9] focus-visible:[outline-offset:2px] active:translate-y-0",
        className,
      )}
      {...props}
    />
  );
}

function CardHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card-header"
      className={cn(
        "@container/card-header grid auto-rows-min grid-rows-[auto_auto] items-start gap-1.5 px-6 has-data-[slot=card-action]:grid-cols-[1fr_auto]",
        className,
      )}
      {...props}
    />
  );
}

// a11y: 見た目は見出しだが既定では <div>。利用側が `as` で h1〜h6 などへ
// 差し替えられるようにする(既定の見た目は変えない)。
function CardTitle({
  className,
  as: Comp = "div",
  ...props
}: React.ComponentProps<"div"> & { as?: React.ElementType }) {
  return (
    <Comp data-slot="card-title" className={cn("font-bold tracking-tight", className)} {...props} />
  );
}

function CardDescription({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card-description"
      className={cn("text-sm text-muted-foreground", className)}
      {...props}
    />
  );
}

function CardAction({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card-action"
      className={cn("col-start-2 row-span-2 row-start-1 self-start justify-self-end", className)}
      {...props}
    />
  );
}

function CardContent({ className, ...props }: React.ComponentProps<"div">) {
  return <div data-slot="card-content" className={cn("px-6", className)} {...props} />;
}

function CardFooter({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div data-slot="card-footer" className={cn("flex items-center px-6", className)} {...props} />
  );
}

export { Card, CardHeader, CardFooter, CardTitle, CardAction, CardDescription, CardContent };
