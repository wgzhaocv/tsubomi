import { Plus } from "lucide-react";

import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Divider } from "@/components/ui/divider";
import { Title } from "@/components/ui/title";
import { Tooltip } from "@/components/ui/tooltip";
import type { ResourceNav } from "@/lib/resources";
import { useCopied } from "@/lib/use-copied";

// リソース一覧画面の共通実装。中身(一覧)は M1〜M5 で実装するため、今は見出し +
// 空状態のみ。1 つの RESOURCES 設定から見出し・色・空状態・作成コマンドが決まる。
export default function ResourcePage({ resource }: { resource: ResourceNav }) {
  const Icon = resource.icon;
  const createHint = resource.createHint;
  const { copied, copy } = useCopied();

  return (
    <div className="flex flex-col gap-7">
      <PageMeta title={resource.label} />

      {/* 見出し:リボン + 補助説明 +(作成は CLI のため)無効の作成ボタン */}
      <header className="flex flex-wrap items-end justify-between gap-4">
        <div className="flex flex-col gap-2.5">
          <Title size="large" color={resource.ribbon}>
            {resource.label}
          </Title>
          <p className="text-sm font-medium text-foreground/70">{resource.tagline}</p>
        </div>
        {createHint && (
          <Tooltip title="作成は CLI から行います">
            <span className="inline-flex">
              <Button type="default" icon={<Plus className="size-4" />} disabled>
                新規作成
              </Button>
            </span>
          </Tooltip>
        )}
      </header>

      <Divider type="line-brown" />

      {/* 空状態:破線カード。原典 Card の dashed と同じ語彙を任意値で再現。 */}
      <Card className="border-2 border-dashed border-[#c4b89e] bg-transparent">
        <CardContent className="flex flex-col items-center gap-4 px-6 py-12 text-center">
          <div className="grid size-16 place-items-center rounded-full bg-accent text-accent-foreground">
            <Icon className="size-8" />
          </div>
          <div className="flex flex-col gap-1.5">
            <p className="text-lg font-bold text-foreground">{resource.emptyTitle}</p>
            <p className="max-w-md text-sm font-medium text-muted-foreground">
              {resource.emptyBody}
            </p>
          </div>

          {createHint && (
            <div className="mt-2 flex w-full max-w-md flex-col items-stretch gap-2">
              <span className="text-xs font-bold text-muted-foreground">CLI で作成:</span>
              <div className="flex items-center gap-2">
                <code className="flex-1 overflow-x-auto rounded-xl bg-secondary px-3 py-2.5 text-left text-xs whitespace-pre text-foreground/90">
                  {createHint}
                </code>
                <Button type="default" size="small" onClick={() => copy(createHint)}>
                  {copied ? "✓" : "コピー"}
                </Button>
              </div>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
