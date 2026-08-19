import { useMemo, useState } from "react";
import { useParams } from "react-router";
import { AxisBottom, AxisLeft } from "@visx/axis";
import { Group } from "@visx/group";
import { ParentSize } from "@visx/responsive";
import { scaleBand, scaleLinear } from "@visx/scale";
import { Bar } from "@visx/shape";

import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Stat } from "@/components/ui/stat";
import { MetricRow } from "@/components/usage-metric";
import { type ServiceStats, type StatsSlice, useServiceStats } from "@/lib/services";

// アクセス統計タブ(doc/paas-service-stats-design.md)。traefik access log 由来の
// リクエスト単位の統計 — Vercel の pageview とは口径が違うので、注記を常に出す。
// チャートは visx(headless)を意匠ごとこちらで指定(既定スタイルを持たないのが採用理由)。

const RANGES = [
  { key: 1, label: "24時間" },
  { key: 7, label: "7日" },
  { key: 30, label: "30日" },
] as const;

// 語彙は wire(英語)のまま来る。表示だけ日本語へ(`?? key` fallback が既定の作法)。
const DEVICE_LABEL: Record<string, string> = {
  desktop: "デスクトップ",
  mobile: "モバイル",
  bot: "bot",
  other: "その他",
};
const UNKNOWN_LABEL: Record<string, string> = { unknown: "不明" };

export default function ServiceStats() {
  const { id = "" } = useParams();
  const [days, setDays] = useState<number>(7);
  const { data, error, isPending } = useServiceStats(id, days);

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-lg font-bold text-foreground">統計</h2>
        <div className="flex gap-2">
          {RANGES.map((r) => (
            <Button
              key={r.key}
              type={days === r.key ? "primary" : "default"}
              size="small"
              onClick={() => setDays(r.key)}
            >
              {r.label}
            </Button>
          ))}
        </div>
      </div>
      <p className="text-sm font-medium text-muted-foreground">
        公開入口(traefik)を通ったリクエストの統計です。静的資産や API 呼び出しも 1
        リクエストと数えます。訪問者は bot を除いた日単位の匿名集計です(IP は保存しません)。
      </p>

      {error && (
        <p className="text-sm font-semibold text-[#e05a5a]">
          読み込みに失敗しました:{error.message}
        </p>
      )}

      {!error && (
        <>
          <Totals data={data} loading={isPending} />
          {data && data.totals.requests === 0 ? (
            <p className="text-sm font-medium text-muted-foreground">
              (期間内のアクセスはまだありません。公開 URL へのアクセスが数秒〜数十秒で 反映されます)
            </p>
          ) : (
            <>
              <TrafficChart data={data} />
              <Breakdowns data={data} />
            </>
          )}
        </>
      )}
    </div>
  );
}

function Totals({ data, loading }: { data: ServiceStats | undefined; loading: boolean }) {
  const v = (f: (d: ServiceStats) => string) =>
    loading || !data ? <Skeleton className="h-4 w-16" /> : f(data);
  return (
    <dl className="grid grid-cols-2 gap-px overflow-hidden rounded-2xl border-2 border-[#e8e2d6] bg-[#e8e2d6] sm:grid-cols-4">
      <Stat label="リクエスト">{v((d) => d.totals.requests.toLocaleString("ja-JP"))}</Stat>
      <Stat label="訪問者(bot 除外)">{v((d) => d.totals.visitors.toLocaleString("ja-JP"))}</Stat>
      <Stat label="bot リクエスト">{v((d) => d.totals.bot_requests.toLocaleString("ja-JP"))}</Stat>
      <Stat label="平均応答">
        {v((d) =>
          d.totals.avg_duration_ms == null ? "—" : `${Math.round(d.totals.avg_duration_ms)}ms`,
        )}
      </Stat>
    </dl>
  );
}

// ===== 推移チャート =====

type Bucket = { t: number; requests: number; visitors: number };

// サーバはイベントの在る刻みしか返さない(0 埋めは表示側の責務 — DTO コメント)。
// 範囲はサーバの from/to(interval 境界揃え・UTC)をそのまま使う — クライアントの時計で
// 窓を再計算すると、時計ずれや rolling 窓の丸めで最古バケットがこぼれる(codex 審査 2026-08-20)。
function zeroFill(data: ServiceStats): Bucket[] {
  const step = data.interval === "hour" ? 3_600_000 : 86_400_000;
  const have = new Map(data.series.map((p) => [Date.parse(p.t), p]));
  const from = Date.parse(data.from);
  const to = Date.parse(data.to);
  const out: Bucket[] = [];
  for (let t = from; t <= to; t += step) {
    const p = have.get(t);
    out.push({ t, requests: p?.requests ?? 0, visitors: p?.visitors ?? 0 });
  }
  return out;
}

function fmtBucket(t: number, interval: "hour" | "day"): string {
  const d = new Date(t);
  return interval === "hour"
    ? d.toLocaleString("ja-JP", { month: "numeric", day: "numeric", hour: "numeric" })
    : d.toLocaleDateString("ja-JP", { month: "numeric", day: "numeric" });
}

function TrafficChart({ data }: { data: ServiceStats | undefined }) {
  const buckets = useMemo(() => (data ? zeroFill(data) : []), [data]);
  const [hover, setHover] = useState<number | null>(null);

  if (!data) {
    return <Skeleton className="h-56 w-full rounded-2xl" />;
  }
  return (
    <div className="flex flex-col gap-2 rounded-2xl border-2 border-[#e8e2d6] bg-card p-4">
      <div className="flex items-center justify-between gap-3">
        <span className="text-sm font-bold text-foreground">推移</span>
        <div className="flex items-center gap-4 text-xs font-semibold text-muted-foreground">
          <span className="flex items-center gap-1.5">
            <span className="inline-block size-2.5 rounded-full bg-[#0CC0B5]" />
            リクエスト
          </span>
          <span className="flex items-center gap-1.5">
            <span className="inline-block size-2.5 rounded-full bg-[#794f27]" />
            訪問者
          </span>
        </div>
      </div>
      <div className="relative h-52 w-full">
        <ParentSize>
          {({ width, height }) =>
            width > 0 && (
              <Chart
                width={width}
                height={height}
                buckets={buckets}
                interval={data.interval}
                hover={hover}
                setHover={setHover}
              />
            )
          }
        </ParentSize>
        {hover != null && buckets[hover] && (
          <div
            className="pointer-events-none absolute top-1 rounded-xl border-2 border-[#e8e2d6] bg-card px-3 py-1.5 text-xs font-semibold text-foreground shadow-sm"
            style={{
              left: `${((hover + 0.5) / buckets.length) * 100}%`,
              transform: hover > buckets.length / 2 ? "translateX(-105%)" : "translateX(5%)",
            }}
          >
            <div className="text-muted-foreground">
              {fmtBucket(buckets[hover].t, data.interval)}
            </div>
            <div>リクエスト {buckets[hover].requests.toLocaleString("ja-JP")}</div>
            <div>訪問者 {buckets[hover].visitors.toLocaleString("ja-JP")}</div>
          </div>
        )}
      </div>
    </div>
  );
}

// visx は headless(スケール計算 + SVG 原語だけ)なので、色・線・文字は全部ここで
// プロジェクトの意匠に合わせる。軸ラベルの間引きは幅から算出(重なり防止)。
function Chart({
  width,
  height,
  buckets,
  interval,
  hover,
  setHover,
}: {
  width: number;
  height: number;
  buckets: Bucket[];
  interval: "hour" | "day";
  hover: number | null;
  setHover: (i: number | null) => void;
}) {
  const margin = { top: 8, right: 8, bottom: 24, left: 40 };
  const xMax = Math.max(0, width - margin.left - margin.right);
  const yMax = Math.max(0, height - margin.top - margin.bottom);
  const maxY = Math.max(1, ...buckets.map((b) => b.requests));

  const x = scaleBand<number>({
    domain: buckets.map((b) => b.t),
    range: [0, xMax],
    padding: 0.25,
  });
  const y = scaleLinear<number>({ domain: [0, maxY], range: [yMax, 0], nice: true });

  const everyNth = Math.max(1, Math.ceil(buckets.length / Math.max(2, Math.floor(xMax / 64))));
  const tickValues = buckets.map((b) => b.t).filter((_, i) => i % everyNth === 0);
  const axisLabel = {
    fill: "#725d42",
    fontSize: 10,
    fontWeight: 600,
    fontFamily: "inherit",
  } as const;

  return (
    <svg width={width} height={height} role="img" aria-label="期間内のリクエスト推移">
      <Group left={margin.left} top={margin.top}>
        {y.ticks(4).map((v) => (
          <line
            key={v}
            x1={0}
            x2={xMax}
            y1={y(v)}
            y2={y(v)}
            stroke="rgba(196,184,158,0.35)"
            strokeWidth={1}
          />
        ))}
        {buckets.map((b, i) => {
          // 2 系列は**並列**の双柱(重ね描きは値が近いと縞模様の 1 本に見えて系列が読めない —
          // ユーザ報告 2026-08-20)。band が広い時(バケット数が少ない疎データ)は上限幅で
          // 中央寄せし、1 本だけの「板」にしない。
          const band = x.bandwidth();
          const bw = Math.min(band, 48);
          const bx = (x(b.t) ?? 0) + (band - bw) / 2;
          const half = bw * 0.46;
          return (
            <Group key={b.t}>
              {/* hover の当たり判定は列全体(細い棒だけだと狙いにくい)。 */}
              <Bar
                x={x(b.t) ?? 0}
                y={0}
                width={band}
                height={yMax}
                fill={hover === i ? "rgba(196,184,158,0.18)" : "transparent"}
                onMouseEnter={() => setHover(i)}
                onMouseLeave={() => setHover(null)}
              />
              <Bar
                x={bx}
                y={y(b.requests)}
                width={half}
                height={yMax - y(b.requests)}
                rx={Math.min(3, half / 2)}
                fill="#0CC0B5"
                pointerEvents="none"
              />
              {b.visitors > 0 && (
                <Bar
                  x={bx + bw - half}
                  y={y(b.visitors)}
                  width={half}
                  height={yMax - y(b.visitors)}
                  rx={Math.min(3, half / 2)}
                  fill="#794f27"
                  pointerEvents="none"
                />
              )}
            </Group>
          );
        })}
        <AxisLeft
          scale={y}
          numTicks={4}
          hideAxisLine
          hideTicks
          tickLabelProps={() => ({ ...axisLabel, textAnchor: "end", dx: -4, dy: 3 })}
        />
        <AxisBottom
          top={yMax}
          scale={x}
          tickValues={tickValues}
          hideAxisLine
          hideTicks
          tickFormat={(t) => fmtBucket(Number(t), interval)}
          tickLabelProps={() => ({ ...axisLabel, textAnchor: "middle", dy: 4 })}
        />
      </Group>
    </svg>
  );
}

// ===== 内訳 =====

function Breakdowns({ data }: { data: ServiceStats | undefined }) {
  if (!data) {
    return (
      <div className="grid gap-4 sm:grid-cols-2">
        {Array.from({ length: 4 }, (_, i) => (
          <Skeleton key={i} className="h-40 w-full rounded-2xl" />
        ))}
      </div>
    );
  }
  const total = Math.max(1, data.totals.requests);
  return (
    <div className="grid gap-4 sm:grid-cols-2">
      <SliceCard title="Top パス" rows={data.top_paths} total={total} mono />
      <SliceCard title="ステータス" rows={data.statuses} total={total} mono />
      <SliceCard title="デバイス" rows={data.devices} total={total} labels={DEVICE_LABEL} />
      <SliceCard title="ブラウザ" rows={data.browsers} total={total} labels={UNKNOWN_LABEL} />
      <SliceCard title="OS" rows={data.oses} total={total} labels={UNKNOWN_LABEL} />
      <SliceCard
        title="国"
        rows={data.countries}
        total={total}
        empty="(前段が Cloudflare のときだけ記録されます)"
      />
      <SliceCard
        title="リファラ"
        rows={data.referers}
        total={total}
        mono
        empty="(Referer 付きのアクセスがまだありません)"
      />
    </div>
  );
}

function SliceCard({
  title,
  rows,
  total,
  labels,
  mono,
  empty,
}: {
  title: string;
  rows: StatsSlice[];
  total: number;
  labels?: Record<string, string>;
  mono?: boolean;
  empty?: string;
}) {
  return (
    <section className="flex flex-col gap-3 rounded-2xl border-2 border-[#e8e2d6] bg-card p-4">
      <h3 className="text-sm font-bold text-foreground">{title}</h3>
      {rows.length === 0 ? (
        <p className="text-xs font-medium text-muted-foreground">
          {empty ?? "(データがありません)"}
        </p>
      ) : (
        <ul className="flex flex-col gap-2.5">
          {rows.map((r) => (
            <li key={r.key}>
              <MetricRow
                label={labels?.[r.key] ?? r.key}
                mono={mono}
                pct={(r.requests / total) * 100}
                detail={r.requests.toLocaleString("ja-JP")}
                loading={false}
              />
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
