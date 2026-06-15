import { Link } from "react-router";
import { BarChart3, type LucideIcon, Server, Users } from "lucide-react";

import { PageContainer } from "@/components/page-container";
import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Divider } from "@/components/ui/divider";
import { Title } from "@/components/ui/title";
import {
  type AdminOverviewKind,
  formatUsageByKind,
  KIND_LABEL,
  useAdminOverview,
} from "@/lib/admin";
import { RESOURCES } from "@/lib/resources";

// 管制面の総覧(owner 専用)。種別ごとの総数 + 総使用量 + 資源保有ユーザ数。
// 匿名化(設計 v2 §7):資源の名前・内容は出さない。owner ゲートは <RequireOwner>
// (router)に集約済み。後端も owner + session を毎回検証。

// kind → アイコン。RESOURCES(単一の真実源)から導出 — サイドメニューと揃える。
const KIND_ICON: Record<string, LucideIcon> = Object.fromEntries(
  RESOURCES.filter((r) => r.kind).map((r) => [r.kind as string, r.icon]),
);

// 使用量の単位(種別で意味が違うことを明示)。service=稼働中内存 / db=存储 / volume=占用 /
// cache=キー数(§4.2。正確なメモリは valkey に無いので key 数を代用)。
const USAGE_LABEL: Record<string, string> = {
  service: "稼働中の内存合計",
  database: "ストレージ合計",
  volume: "占用合計",
  cache: "キー数合計",
};

function KindCard({ k }: { k: AdminOverviewKind }) {
  const Icon = KIND_ICON[k.kind] ?? Server;
  return (
    <Card>
      <CardContent className="flex flex-col gap-3">
        <div className="flex items-center gap-3">
          <div className="grid size-11 shrink-0 place-items-center rounded-2xl bg-accent text-accent-foreground">
            <Icon className="size-5.5" />
          </div>
          <div className="flex min-w-0 flex-col">
            <span className="text-base font-bold text-foreground">
              {KIND_LABEL[k.kind] ?? k.kind}
            </span>
            <span className="text-xs font-medium text-muted-foreground">
              {USAGE_LABEL[k.kind] ?? "使用量"}
            </span>
          </div>
        </div>
        <div className="flex items-end justify-between gap-3">
          <span className="text-3xl font-extrabold tracking-tight text-foreground">
            {k.count}
            <span className="ml-1 text-sm font-semibold text-muted-foreground">個</span>
          </span>
          <span className="font-mono text-lg font-bold text-[#0b9c93]">
            {formatUsageByKind(k.kind, k.total_usage_bytes)}
          </span>
        </div>
      </CardContent>
    </Card>
  );
}

export default function AdminOverview() {
  const { data, isPending, error } = useAdminOverview();

  return (
    <PageContainer>
      <div className="flex flex-col gap-7">
        <PageMeta title="管制面の総覧" />

        <header className="flex flex-wrap items-center justify-between gap-4">
          <Title size="large" color="purple">
            管制面の総覧
          </Title>
          <Button type="default" asChild>
            <Link to="/admin/ranking" className="inline-flex items-center gap-2">
              <BarChart3 className="size-4" />
              使用量ランキング
            </Link>
          </Button>
        </header>

        <Divider type="line-brown" />

        <p className="max-w-2xl text-sm font-medium text-muted-foreground">
          全ユーザの資源と使用量の総覧です。資源の名前や中身は表示されません(誰が・何種類・
          どれだけ使っているかだけ)。
        </p>

        {error && (
          <p className="text-sm font-semibold text-[#e05a5a]">
            読み込みに失敗しました:{error.message}
          </p>
        )}

        {isPending && !data && (
          <p className="text-sm font-medium text-muted-foreground">読み込み中…</p>
        )}

        {data && (
          <>
            <Card>
              <CardContent className="flex items-center gap-3.5">
                <div className="grid size-11 shrink-0 place-items-center rounded-2xl bg-accent text-accent-foreground">
                  <Users className="size-5.5" />
                </div>
                <div className="flex flex-col">
                  <span className="text-2xl font-extrabold tracking-tight text-foreground">
                    {data.user_count}
                    <span className="ml-1 text-sm font-semibold text-muted-foreground">名</span>
                  </span>
                  <span className="text-xs font-medium text-muted-foreground">
                    資源を持つ利用者
                  </span>
                </div>
              </CardContent>
            </Card>

            <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
              {data.kinds.map((k) => (
                <KindCard key={k.kind} k={k} />
              ))}
            </div>
          </>
        )}
      </div>
    </PageContainer>
  );
}
