import { Link } from "react-router";
import { BarChart3, Boxes, type LucideIcon, Server, Users } from "lucide-react";

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
import { type HostMetrics, useHostMetrics } from "@/lib/host-metrics";
import { RESOURCES } from "@/lib/resources";
import { formatBytes } from "@/lib/volumes";

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

// 概要に並べる種別の固定順(後端 KINDS と一致)。骨架も実データも同じ順で描く。
const KIND_ORDER = ["service", "database", "volume", "cache"] as const;

// 種別カード。`row` が null(読み込み中)なら数字は「—」を出す — 骨架を最初から描いて
// データ到着で数字だけ差し替える(spinner→カードの差し替えで起きるレイアウト抖動を無くす)。
function KindCard({ kind, row }: { kind: string; row: AdminOverviewKind | null }) {
  const Icon = KIND_ICON[kind] ?? Server;
  return (
    <Card>
      <CardContent className="flex flex-col gap-3">
        <div className="flex items-center gap-3">
          <div className="grid size-11 shrink-0 place-items-center rounded-2xl bg-accent text-accent-foreground">
            <Icon className="size-5.5" />
          </div>
          <div className="flex min-w-0 flex-col">
            <span className="text-base font-bold text-foreground">
              {KIND_LABEL[kind] ?? kind}
            </span>
            <span className="text-xs font-medium text-muted-foreground">
              {USAGE_LABEL[kind] ?? "使用量"}
            </span>
          </div>
        </div>
        <div className="flex items-end justify-between gap-3">
          <span className="text-3xl font-extrabold tracking-tight text-foreground">
            {row ? row.count : "—"}
            <span className="ml-1 text-sm font-semibold text-muted-foreground">個</span>
          </span>
          <span className="font-mono text-lg font-bold text-[#0b9c93]">
            {row ? formatUsageByKind(kind, row.total_usage_bytes) : "—"}
          </span>
        </div>
      </CardContent>
    </Card>
  );
}

// 用量バー(VolumeFileBrowser のアップロード進捗バーと同じ意匠)。pct が null なら 0 幅。
function UsageBar({ pct }: { pct: number | null }) {
  return (
    <div className="h-2 w-full overflow-hidden rounded-full bg-[rgba(196,184,158,0.3)]">
      <div
        className="h-full rounded-full bg-[#0CC0B5] transition-[width] duration-150 ease-out"
        style={{ width: `${Math.min(100, Math.max(0, pct ?? 0))}%` }}
      />
    </div>
  );
}

function MetricRow({ label, pct, detail }: { label: string; pct: number | null; detail: string }) {
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-baseline justify-between gap-3">
        <span className="text-sm font-bold text-foreground">{label}</span>
        <span className="font-mono text-sm font-bold text-[#0b9c93]">{detail}</span>
      </div>
      <UsageBar pct={pct} />
    </div>
  );
}

// used / total を「1.2 GB / 8.0 GB」に。どちらか欠ければ「—」(dev macOS は /proc 無しで null)。
function formatPair(used: number | null | undefined, total: number | null | undefined): string {
  if (used == null || total == null) return "—";
  return `${formatBytes(used)} / ${formatBytes(total)}`;
}

// サーバー本体(ホスト)の使用状況。データは WS(useHostMetrics)で 5s 毎に更新。来る前 /
// 取得不能(dev の CPU・メモリ)は「—」と 0 幅バー。HTTP の overview とは独立。
function HostCard({ data, connected }: { data: HostMetrics | null; connected: boolean }) {
  const cpu = data?.cpu_pct ?? null;
  const memPct =
    data?.mem_used != null && data.mem_total ? (data.mem_used / data.mem_total) * 100 : null;
  const diskPct = data?.disk_pct ?? null;

  return (
    <Card>
      <CardContent className="flex flex-col gap-4">
        <div className="flex items-center gap-3.5">
          <div className="grid size-11 shrink-0 place-items-center rounded-2xl bg-accent text-accent-foreground">
            <Server className="size-5.5" />
          </div>
          <div className="flex flex-col">
            <span className="text-base font-bold text-foreground">サーバー</span>
            <span className="text-xs font-medium text-muted-foreground">
              本体(ホスト)の使用状況 · {connected ? "約 5 秒ごとに更新" : "接続待ち…"}
            </span>
          </div>
        </div>
        <div className="flex flex-col gap-3.5">
          <MetricRow label="CPU" pct={cpu} detail={cpu == null ? "—" : `${cpu.toFixed(0)}%`} />
          <MetricRow
            label="メモリ"
            pct={memPct}
            detail={formatPair(data?.mem_used, data?.mem_total)}
          />
          <MetricRow
            label="ディスク"
            pct={diskPct}
            detail={formatPair(data?.disk_used, data?.disk_total)}
          />
        </div>
      </CardContent>
    </Card>
  );
}

// プラットフォーム自身(server + infra コンテナ)の使用量を**各コンテナ別**に出す。
// 加総せず一覧 — どの基礎設施が重いか分かる。用户 app は含めない。dev は server が
// 容器でないので並ばない(infra のみ)。データは HostCard と同じ WS から(props で受ける)。
function PlatformCard({ items }: { items: HostMetrics["platform"] }) {
  return (
    <Card>
      <CardContent className="flex flex-col gap-4">
        <div className="flex items-center gap-3.5">
          <div className="grid size-11 shrink-0 place-items-center rounded-2xl bg-accent text-accent-foreground">
            <Boxes className="size-5.5" />
          </div>
          <div className="flex flex-col">
            <span className="text-base font-bold text-foreground">プラットフォーム自身</span>
            <span className="text-xs font-medium text-muted-foreground">
              各コンテナの CPU / メモリ(利用者のアプリは含みません)· 約 5 秒ごとに更新
            </span>
          </div>
        </div>
        {items.length === 0 ? (
          <span className="text-sm font-medium text-muted-foreground">
            コンテナ情報がありません(取得待ち、または dev では server は対象外)。
          </span>
        ) : (
          <dl className="flex flex-col divide-y divide-[rgba(196,184,158,0.3)]">
            {items.map((c) => (
              <div key={c.name} className="flex items-center justify-between gap-3 py-2.5">
                <dt className="font-mono text-sm font-bold text-foreground">{c.name}</dt>
                <dd className="flex items-center gap-5">
                  <span className="text-sm tabular-nums text-muted-foreground">
                    CPU {c.cpu_pct == null ? "—" : `${c.cpu_pct.toFixed(0)}%`}
                  </span>
                  <span className="w-20 text-right font-mono text-sm font-bold text-[#0b9c93]">
                    {formatBytes(c.mem_bytes)}
                  </span>
                </dd>
              </div>
            ))}
          </dl>
        )}
      </CardContent>
    </Card>
  );
}

export default function AdminOverview() {
  const { data, error } = useAdminOverview();
  // ホスト指標 WS は**この 1 箇所だけ**で開く(HostCard / PlatformCard に props で配る)。
  // 2 回呼ぶと WS が 2 本張られるため。
  const host = useHostMetrics();

  return (
    <PageContainer>
      <div className="flex flex-col gap-7">
        <PageMeta title="リソース概要" />

        <header className="flex flex-wrap items-center justify-between gap-4">
          <Title size="large" color="purple">
            リソース概要
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

        <HostCard data={host.data} connected={host.connected} />

        {error && (
          <p className="text-sm font-semibold text-[#e05a5a]">
            読み込みに失敗しました:{error.message}
          </p>
        )}

        {/* カード骨架は最初から描き、読み込み中は数字を「—」にする(spinner→カードの
            差し替えで起きるレイアウト抖動を防ぐ。host カードと同じ作法)。error 時は
            上のメッセージだけ出してカードは出さない。 */}
        {!error && (
          <>
            <Card>
              <CardContent className="flex items-center gap-3.5">
                <div className="grid size-11 shrink-0 place-items-center rounded-2xl bg-accent text-accent-foreground">
                  <Users className="size-5.5" />
                </div>
                <div className="flex flex-col">
                  <span className="text-2xl font-extrabold tracking-tight text-foreground">
                    {data ? data.user_count : "—"}
                    <span className="ml-1 text-sm font-semibold text-muted-foreground">名</span>
                  </span>
                  <span className="text-xs font-medium text-muted-foreground">
                    資源を持つ利用者
                  </span>
                </div>
              </CardContent>
            </Card>

            <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
              {KIND_ORDER.map((kind) => (
                <KindCard
                  key={kind}
                  kind={kind}
                  row={data?.kinds.find((k) => k.kind === kind) ?? null}
                />
              ))}
            </div>
          </>
        )}

        {/* 最下部:プラットフォーム自身(server + infra)のコンテナ別使用量。 */}
        <PlatformCard items={host.data?.platform ?? []} />
      </div>
    </PageContainer>
  );
}
