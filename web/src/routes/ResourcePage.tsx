import { Plus } from "lucide-react";

import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Divider } from "@/components/ui/divider";
import { Title } from "@/components/ui/title";
import type { ResourceNav } from "@/lib/resources";

// リソース一覧画面の共通実装(サービス / ボリューム / キャッシュ / アクティビティ)。
// 中身(一覧・作成)は今後 web で実装する。今は見出し + 空状態のみ。1 つの RESOURCES
// 設定から見出し・色・空状態が決まる。横幅はデータベース詳細と揃えて wide。
export default function ResourcePage({ resource }: { resource: ResourceNav }) {
  const Icon = resource.icon;

  return (
    <PageContainer width="wide">
      <div className="flex flex-col gap-7">
        <PageMeta title={resource.label} />

        {/* 見出し:リボン + 新規作成(配線は後日)。行 flex なのでリボンは内容幅のまま。 */}
        <header className="flex flex-wrap items-center justify-between gap-4">
          <Title size="large" color={resource.ribbon}>
            {resource.label}
          </Title>
          <Button type="default" icon={<Plus className="size-4" />} disabled>
            新規作成
          </Button>
        </header>

        <Divider type="line-brown" />

        {/* 空状態:共有 Card の dashed バリアント(実色クリーム + 破線枠)。 */}
        <Card type="dashed">
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
          </CardContent>
        </Card>
      </div>
    </PageContainer>
  );
}
