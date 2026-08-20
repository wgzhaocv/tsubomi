import { useState } from "react";
import {
  Check,
  Copy,
  ExternalLink,
  EyeOff,
  Globe,
  Pencil,
  Play,
  Square,
  Trash2,
} from "lucide-react";
import { Link, useNavigate, useParams } from "react-router";

import { Badge } from "@/components/phase-badge";
import { Button } from "@/components/ui/button";
import { Divider } from "@/components/ui/divider";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { Radio } from "@/components/ui/radio";
import { MetricRow } from "@/components/usage-metric";
import { formatBytesPair, formatRelative } from "@/lib/format";
import {
  deployStatusLabel,
  desiredLabel,
  phaseLabel,
  type ServiceCaller,
  type ServiceMetrics,
  serviceVisibility,
  type SetLimitsInput,
  shortDigest,
  useDeleteService,
  useService,
  useServiceCallers,
  useServiceMetrics,
  useSetServiceLimits,
  useSetServiceVisibility,
  useSetSubdomain,
  useStartService,
  useStopService,
  VISIBILITY_OPTIONS,
} from "@/lib/services";
import { useCopied } from "@/lib/use-copied";
import { cn } from "@/lib/utils";

// 概要:状態 grid + 操作(開始 / 停止)+ 公開範囲(Radio 3 択)+ 危険ゾーン(削除 = 名前入力確認)。
// 操作は再デプロイ(start-first)を伴うので結果が返るまで loading。
export default function ServiceOverview() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const { data: svc } = useService(id);
  const start = useStartService(id);
  const stop = useStopService(id);
  const del = useDeleteService(id);
  const setVis = useSetServiceVisibility(id);
  const setSub = useSetSubdomain(id);
  // 「誰が私を注入しているか」= 改名の影響範囲。常設セクションと変更 modal が同じ配列を読む。
  // **未知(取得前 / 失敗)を 0 件と同一視しない** — 改名 modal は影響範囲を言えないまま
  // 通してはいけないので、状態も持つ(codex 審査)。
  const {
    data: callers,
    isFetching: callersFetching,
    error: callersError,
    refetch: refetchCallers,
  } = useServiceCallers(id);
  // 稼働中の呼び出し側だけが「今の値が凍結されている」= 改名で内部リンクが切れる相手。
  // 停止中 / 未デプロイは凍結 env も生きたリンクも無いので、次の起動で新しい値が入るだけ。
  const runningCallers = callers?.filter((c) => c.desired_state === "running").length ?? 0;

  const [deleteOpen, setDeleteOpen] = useState(false);
  const [confirmName, setConfirmName] = useState("");
  // subdomain 編集 modal(rename modal と同型)。Outlet は id で key されるので
  // ここでは id 変化時の強制クローズは不要(遷移でコンポーネントごと作り直される)。
  const [subOpen, setSubOpen] = useState(false);
  const [subValue, setSubValue] = useState("");
  const submitSubdomain = () => {
    const trimmed = subValue.trim();
    // 二重送信を防ぐ + **影響範囲の取得中は待つ**(名単を出せないまま改名させない)。
    // 取得**失敗**は塞がない — 補助的な読みの不調で主操作を止めるのは行き過ぎなので、
    // 代わりに modal で「確認できなかった」と明示する(下の警告文)。
    if (!trimmed || setSub.isPending || callersFetching) return;
    setSub.mutate(trimmed, { onSuccess: () => setSubOpen(false) });
  };
  const { copied, copy } = useCopied();
  // url を局所定数に取り出して narrow する(onClick クロージャ内でも string 確定にする)。
  const url = svc?.url;
  const urlText = url?.replace(/^https?:\/\//, "");
  const visibility = serviceVisibility(svc);
  const isPrivate = visibility === "private";
  const actionErr = start.error ?? stop.error;
  // svc 未取得 / どちらかの操作が進行中なら両ボタンを止める(未知状態への発火・start と stop の同時発火を防ぐ)。
  const busy = !svc || start.isPending || stop.isPending;

  return (
    <div className="flex flex-col gap-7">
      {/* ===== 公開 URL(目立つ位置に独立表示。クリックで開く / コピー)=====
          private 中は**消さずに灰色化**して「非公開中」を明示 — subdomain は温存されており、
          再公開すれば同じ URL で復活するため。URL 文字列とコピーは残し、「開く」は /noservice に
          飛ぶだけなので出さない。 */}
      {url && (
        <section
          className={cn(
            "flex flex-wrap items-center gap-3 rounded-2xl border-2 px-5 py-4",
            isPrivate ? "border-[#e8e2d6] bg-card" : "border-[#19c8b9]/35 bg-accent",
          )}
        >
          <div
            className={cn(
              "grid size-11 shrink-0 place-items-center rounded-2xl",
              isPrivate
                ? "bg-[#e8e2d6]/60 text-muted-foreground"
                : "bg-[#19c8b9]/15 text-[#11a89b]",
            )}
          >
            {isPrivate ? <EyeOff className="size-5.5" /> : <Globe className="size-5.5" />}
          </div>
          <div className="flex min-w-0 flex-1 flex-col">
            <span className="text-xs font-bold text-muted-foreground">
              {isPrivate ? "公開 URL(非公開中 — 外部からはアクセスできません)" : "公開 URL"}
            </span>
            {isPrivate ? (
              <span className="truncate text-base font-bold text-muted-foreground">{urlText}</span>
            ) : (
              <a
                href={url}
                target="_blank"
                rel="noreferrer"
                className="truncate text-base font-bold text-[#11a89b] underline-offset-2 outline-none hover:underline focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-2"
              >
                {urlText}
              </a>
            )}
          </div>
          {/* 狭い画面では w-full で 2 行目へ落とす(flex-1 の URL 列は basis 0 なので、
              放っておくとボタンより先に潰れて「wg…」だけになる)。 */}
          <div className="flex w-full shrink-0 items-center gap-2 sm:w-auto">
            <Button
              type="default"
              size="small"
              icon={copied ? <Check className="size-4" /> : <Copy className="size-4" />}
              onClick={() => copy(url)}
            >
              {copied ? "コピー済み" : "コピー"}
            </Button>
            {!isPrivate && (
              <Button type="primary" size="small" asChild>
                <a href={url} target="_blank" rel="noreferrer">
                  <ExternalLink className="size-4" />
                  開く
                </a>
              </Button>
            )}
          </div>
        </section>
      )}

      {/* ===== 状態 ===== */}
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-bold text-foreground">状態</h2>
        <dl className="grid grid-cols-2 gap-px overflow-hidden rounded-2xl border-2 border-[#e8e2d6] bg-[#e8e2d6] sm:grid-cols-3">
          <Stat label="現在の状態">{svc ? phaseLabel(svc.phase) : "…"}</Stat>
          <Stat label="希望状態">{svc ? desiredLabel(svc.desired_state) : "…"}</Stat>
          <Stat label="ポート">{svc?.container_port ?? "…"}</Stat>
          <Stat label="サブドメイン">
            <span className="flex items-center gap-1">
              <span className="truncate">{svc?.subdomain ?? "…"}</span>
              {/* 編集 = 公開 URL が変わる操作なので modal で警告を添える(下の Modal)。 */}
              <Button
                type="text"
                size="small"
                aria-label="サブドメインを変更"
                icon={<Pencil className="size-3.5" />}
                disabled={!svc}
                onClick={() => {
                  setSubValue(svc?.subdomain ?? "");
                  setSub.reset();
                  // 影響範囲は**開いた瞬間に取り直す**:別タブ / CLI で注入された分を
                  // 取りこぼした古い名単のまま改名させない(codex 審査)。
                  void refetchCallers();
                  setSubOpen(true);
                }}
              />
            </span>
          </Stat>
          <Stat label="イメージ">
            {svc?.image_digest ? shortDigest(svc.image_digest) : "未デプロイ"}
          </Stat>
          <Stat label="最終デプロイ">
            {svc?.last_deploy_at ? new Date(svc.last_deploy_at).toLocaleString("ja-JP") : "—"}
          </Stat>
          <Stat label="メモリ上限">{svc?.memory_mb != null ? `${svc.memory_mb} MiB` : "—"}</Stat>
          <Stat label="CPU 上限">
            {svc?.cpu_limit_millis != null
              ? `${svc.cpu_limit_millis / 1000} CPU`
              : "なし(相対的な重み付けのみ)"}
          </Stat>
          {/* 「ステートフル」は用語だけでは何が変わるか伝わらない(非エンジニア向け)。
              実際に変わるのは**デプロイのやり方**なので、そちらを主に据えて用語は括弧で残す
              (CLI の `tbm service stateful` と結び付けられるように)。 */}
          <Stat label="デプロイ方式">
            {svc?.stateful ? "停止してから入替(ステートフル)" : svc ? "無瞬断で入替" : "…"}
          </Stat>
        </dl>
      </section>

      <Divider type="line-brown" />

      {/* ===== リソース上限(次のデプロイから反映)=====
          svc 確定後に条件レンダー = useState 初期化子で現値を seed できる(render 中 setState 不要)。 */}
      {svc && <LimitsSection svc={svc} id={id} />}

      <Divider type="line-brown" />

      {/* ===== 操作 ===== */}
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-bold text-foreground">操作</h2>
        <p className="text-sm font-medium text-muted-foreground">
          停止するとコンテナを止め、ルートを外します。開始は最後に成功したデプロイのイメージで再起動します。
        </p>
        <div className="flex flex-wrap gap-2">
          <Button
            type="primary"
            icon={<Play className="size-4" />}
            loading={start.isPending}
            disabled={busy || svc?.desired_state === "running"}
            onClick={() => start.mutate()}
          >
            開始
          </Button>
          <Button
            type="default"
            icon={<Square className="size-4" />}
            loading={stop.isPending}
            disabled={busy || svc?.desired_state === "stopped"}
            onClick={() => stop.mutate()}
          >
            停止
          </Button>
        </div>
        {actionErr && <p className="text-sm font-semibold text-[#e05a5a]">{actionErr.message}</p>}
      </section>

      <Divider type="line-brown" />

      {/* ===== 公開範囲(即時反映・再デプロイ不要)===== */}
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-bold text-foreground">公開範囲</h2>
        <p className="text-sm font-medium text-muted-foreground">
          切り替えは即時反映(再デプロイ不要)。非公開にしても内部リンク・ログ・ターミナルは従来どおり使えます。一般公開は
          IP 制限が外れます — アプリ側の認証にご注意ください。
        </p>
        <Radio
          aria-label="公開範囲"
          value={visibility}
          disabled={!svc || setVis.isPending}
          options={[...VISIBILITY_OPTIONS]}
          onChange={(v) => setVis.mutate(String(v))}
        />
        {setVis.error && (
          <p className="text-sm font-semibold text-[#e05a5a]">{setVis.error.message}</p>
        )}
      </section>

      {/* ===== 呼び出し側(このサービスを注入している別のサービス)=====
          0 件なら**セクションごと出さない** — 大半のサービスには呼び出し側が居ないので、
          常設の空セクションは雑音になる。 */}
      {callers && callers.length > 0 && (
        <>
          <Divider type="line-brown" />
          <section className="flex flex-col gap-3">
            <h2 className="text-lg font-bold text-foreground">呼び出し側</h2>
            <p className="text-sm font-medium text-muted-foreground">
              このサービスを注入している(内部リンクで呼んでいる)サービスです。サブドメインを変えると、
              これらのコンテナ内に凍結された接続先は旧サブドメインのままになるため、再デプロイするまで
              内部リンクが切れます。
            </p>
            <ul className="flex flex-col gap-2">
              {callers.map((c) => (
                <CallerItem key={c.id} caller={c} />
              ))}
            </ul>
          </section>
        </>
      )}

      <Divider type="line-brown" />

      {/* ===== 危険ゾーン ===== */}
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-bold text-[#c94444]">削除</h2>
        <p className="text-sm font-medium text-muted-foreground">
          削除するとコンテナを止めてゴミ箱に入ります(3 日間は復元可能)。
        </p>
        <Button
          type="default"
          danger
          icon={<Trash2 className="size-4" />}
          className="w-fit"
          onClick={() => {
            setConfirmName("");
            setDeleteOpen(true);
          }}
        >
          このサービスを削除
        </Button>
      </section>

      {/* 削除確認(名前入力) */}
      <Modal
        open={deleteOpen}
        title="サービスを削除"
        typewriter={false}
        width={460}
        onClose={() => setDeleteOpen(false)}
        footer={
          <>
            <Button type="text" onClick={() => setDeleteOpen(false)}>
              キャンセル
            </Button>
            <Button
              type="primary"
              danger
              loading={del.isPending}
              disabled={confirmName !== svc?.display_name}
              onClick={() =>
                del.mutate(undefined, {
                  onSuccess: () => {
                    setDeleteOpen(false);
                    navigate("/services");
                  },
                })
              }
            >
              削除する
            </Button>
          </>
        }
      >
        <div className="flex w-full flex-col gap-3">
          <p>
            確認のため、サービス名 <strong>{svc?.display_name}</strong> を入力してください。
          </p>
          <Input
            value={confirmName}
            autoFocus
            placeholder={svc?.display_name}
            onChange={(e) => setConfirmName(e.target.value)}
          />
          {del.error && <p className="text-sm font-semibold text-[#e05a5a]">{del.error.message}</p>}
        </div>
      </Modal>

      {/* subdomain 変更(rename modal と同型)。公開 URL が変わる操作なので影響を明記する。 */}
      <Modal
        open={subOpen}
        title="サブドメインを変更"
        typewriter={false}
        width={460}
        onClose={() => setSubOpen(false)}
        footer={
          <>
            <Button type="text" onClick={() => setSubOpen(false)}>
              キャンセル
            </Button>
            <Button
              type="primary"
              loading={setSub.isPending || callersFetching}
              disabled={!subValue.trim() || subValue.trim() === svc?.subdomain || callersFetching}
              onClick={submitSubdomain}
            >
              変更
            </Button>
          </>
        }
      >
        <form
          onSubmit={(e) => {
            e.preventDefault();
            submitSubdomain();
          }}
          className="flex w-full flex-col gap-3"
        >
          <Input
            label="サブドメイン"
            value={subValue}
            autoFocus
            onChange={(e) => setSubValue(e.target.value)}
            description="小文字英数と「-」・英字始まり・「-」終わり不可・50 字以内(予約語と tsubomi- 始まりは不可)。公開 URL が新しいサブドメインに変わります。"
          />
          <p className="text-sm font-medium text-muted-foreground">
            旧 URL は即座に無効になります。GitHub リポジトリ名は変わりません。
          </p>
          {/* 影響範囲は**実際に注入している呼び出し側が居るときだけ**言う。0 件で出すと
              大半のサービスで無関係な脅し文になる(この modal の主目的)。ただし
              **「未知」を「0 件」と同一視しない** — 取得できていないときは黙るのではなく
              確認できなかったと言う(黙ると警告なしで改名が通る = 旧実装より悪い)。 */}
          {callersError ? (
            <p className="text-sm font-semibold text-[#b5862a]">
              呼び出し側(このサービスを注入しているサービス)を確認できませんでした:
              {callersError.message}
              <br />
              注入している呼び出し側がある場合、改名後に再デプロイするまで旧サブドメインを参照し続けます。
            </p>
          ) : callersFetching && !callers ? (
            <p className="text-sm font-medium text-muted-foreground">
              呼び出し側への影響を確認しています…
            </p>
          ) : callers && callers.length > 0 ? (
            <div className="flex flex-col gap-2 bg-card px-3 py-2">
              <p className="text-sm font-bold text-foreground">
                このサービスを注入している呼び出し側 {callers.length} 件
              </p>
              <ul className="flex flex-col gap-2">
                {callers.map((c) => (
                  <CallerItem key={c.id} caller={c} />
                ))}
              </ul>
              {/* 断定を稼働中の相手だけに絞る:停止中 / 未デプロイの呼び出し側には凍結された
                  接続先も生きたリンクも無いので「切れる」は嘘になる(codex 審査)。 */}
              <p className="text-xs font-medium text-muted-foreground">
                {runningCallers > 0
                  ? `稼働中の ${runningCallers} 件は、再デプロイするまで旧サブドメインを参照し続けます(内部リンクが切れます)。停止中の呼び出し側は次に起動したときに新しい値が入ります。`
                  : "いずれも稼働していないので、次に起動したときに新しい値が入ります。"}
              </p>
            </div>
          ) : null}
          {setSub.error && (
            <p className="text-sm font-semibold text-[#e05a5a]">{setSub.error.message}</p>
          )}
        </form>
      </Modal>
    </div>
  );
}

// 呼び出し側 1 件(常設セクションと subdomain 変更 modal が共有)。env 名は集約済み =
// 1 行 1 サービス。裸の値を置かず「環境変数 …」と名詞を付ける。
// 状態の文言は lib の単一真源(desiredLabel / deployStatusLabel)を引く — ここで直書きすると
// 同じ wire 値がページ内で 2 通りの日本語になる。
function CallerItem({ caller }: { caller: ServiceCaller }) {
  return (
    <li className="flex flex-col gap-0.5">
      <div className="flex flex-wrap items-baseline gap-x-2">
        {/* このセクションの用途は「ここへ行って再デプロイする」なので名前はリンクにする
            (ServiceEnv の注入元リンクと同じ作法)。 */}
        <Link
          to={`/services/${caller.id}`}
          className="text-sm font-bold text-[#11a89b] underline-offset-2 outline-none hover:underline focus-visible:[outline:2px_solid_#19c8b9] focus-visible:outline-offset-2"
        >
          {caller.display_name}
        </Link>
        <span className="text-xs font-medium text-muted-foreground">
          注入名 {caller.env_vars.join(", ")}
        </span>
        {/* リンクを切る前に知りたい情報 — 停止中 / 既に直近のデプロイが失敗している呼び出し側。 */}
        {caller.desired_state === "stopped" && (
          <Badge tone="muted" size="sm">
            {desiredLabel(caller.desired_state)}
          </Badge>
        )}
        {caller.last_deploy_status === "failed" && (
          <Badge tone="danger" size="sm">
            直近デプロイ{deployStatusLabel(caller.last_deploy_status)}
          </Badge>
        )}
      </div>
      {caller.last_deploy_error && (
        <span className="text-xs font-semibold text-[#e05a5a]">{caller.last_deploy_error}</span>
      )}
    </li>
  );
}

// 状態グリッドの 1 セル(DatabaseOverview と同じ作法)。
function Stat({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1 bg-card px-4 py-3">
      <dt className="text-xs font-semibold text-muted-foreground">{label}</dt>
      <dd className="truncate text-sm font-bold text-foreground">{children}</dd>
    </div>
  );
}

// 実行中コンテナの使用量(上限を決めるための材料)。上限の入力欄の**上**に置く —
// 「95% まで来ている / OOM で落ちた」を見てから上限を触る、という順序にするため。
// docker の CPU% は 100% = 1 コアなので、`--cpus` と同じ **コア数**に直して見せる
// (単位が揃っていないと入力欄の値と突き合わせられない)。
function CurrentUsage({ id }: { id: string }) {
  // 輪詢の判断はフックの中(lib/services.ts)。同じ key を親も呼ぶが query は 1 本に収束する。
  const { data: m, isLoading, isError } = useServiceMetrics(id);
  // 補助情報なので、取得できない環境(未対応サーバ等)では黙って出さない。
  if (isError) return null;
  if (!isLoading && m && !m.running) {
    return (
      <p className="text-sm font-medium text-muted-foreground">
        コンテナは停止中です(使用量は取得できません)。
      </p>
    );
  }

  const memPct =
    m?.mem_bytes != null && m.mem_limit_bytes != null
      ? (m.mem_bytes / m.mem_limit_bytes) * 100
      : null;
  // docker の CPU% は **100% = 1 コア**。コア数の生値は「多いのか少ないのか」を語らない
  // (8 コア機の 4 コアは全体の半分)ので、**天井に対する割合**を主に据え、絶対値は括弧に落とす。
  // 天井は「今のコンテナに適用されている上限」があればそれ、無ければホスト全体 —— どちらの
  // 状態でも必ず百分率が出る(以前は個別上限が無いとき分母が無く、既定では棒も % も出ていなかった)。
  // 分母に DB の設定値を使ってはいけない:上限を変えて未デプロイのとき、実際は適用済み上限の
  // 100% なのに「上限の 50%」と嘘をつく(未反映は下の pendingNote が別に言う)。
  const cores = m?.cpu_pct != null ? m.cpu_pct / 100 : null;
  const cpuLimitCores = m?.cpu_limit_millis != null ? m.cpu_limit_millis / 1000 : null;
  const hostCores = m?.host_cores ?? null;
  const cpuCeil = cpuLimitCores ?? hostCores;
  const cpuPct = cores != null && cpuCeil ? (cores / cpuCeil) * 100 : null;
  const facts = usageFacts(m);

  return (
    <div className="flex flex-col gap-3 rounded-2xl border-2 border-[#e8e2d6] bg-card p-4">
      <MetricRow
        label="メモリ使用量"
        pct={memPct ?? undefined}
        detail={
          memPct != null
            ? `上限の ${Math.round(memPct)}%(${formatBytesPair(m?.mem_bytes, m?.mem_limit_bytes)})`
            : "—"
        }
        loading={isLoading}
      />
      <MetricRow
        label="CPU 使用量"
        pct={cpuPct ?? undefined}
        detail={
          cores == null
            ? "—"
            : cpuCeil
              ? `${cpuLimitCores ? "上限" : "全体"}の ${Math.round(cpuPct ?? 0)}%(${cores.toFixed(2)} / ${cpuCeil} コア${cpuLimitCores ? "" : "・個別上限なし"})`
              : `${cores.toFixed(2)} コア相当`
        }
        loading={isLoading}
      />
      {facts && <p className="text-xs font-medium text-muted-foreground">{facts}</p>}
    </div>
  );
}

// 使用量バーに添える短い事実(再起動回数 / 起動からの経過 / 直近 OOM)。値が無いものは出さない。
function usageFacts(m?: ServiceMetrics): string {
  if (!m) return "";
  const parts: string[] = [];
  if (m.restart_count != null) parts.push(`再起動 ${m.restart_count} 回`);
  if (m.started_at) parts.push(`起動 ${formatRelative(m.started_at)}`);
  if (m.oom_killed) parts.push("直近の終了は OOM(メモリ不足)");
  return parts.join(" · ");
}

// CPU 上限の入力欄に添える一行。コア数だけでは「機械のどれくらいか」が分からないので、
// 入力値をホスト全体に対する割合へ換算して見せる(空欄 = 上限なしの意味も明示)。
// **保存されるのは常にコア数(絶対値)**で、この % は表示だけ — 割合を保存すると
// 別のコア数のホストへ移した瞬間に同じ設定が別の意味になる。
function cpuHint(cpus: string, hostCores: number | null): string {
  const t = cpus.trim();
  if (t === "") return "空欄 = 上限なし(他の app と相対的に分け合う)";
  const c = Number(t);
  // 不正値は保存時の検証が言う。コア数が取れないときは換算できない。どちらも黙る
  // (代わりに何か喋ると、入力の反響か無関係な豆知識になる)。
  if (!Number.isFinite(c) || c <= 0 || !hostCores) return "";
  return `${c} コア = このホスト(${hostCores} コア)全体の ${Math.round((c / hostCores) * 100)}%`;
}

// リソース上限の変更(memory / cpus)。値は次のデプロイから反映 — 実行中のコンテナには影響しない
// (server の run_digest がデプロイのたびに DB から読み直す)。cpus 欄は空 = 上限なし。
function LimitsSection({
  svc,
  id,
}: {
  svc: NonNullable<ReturnType<typeof useService>["data"]>;
  id: string;
}) {
  const setLimits = useSetServiceLimits(id);
  // 同じ query key を CurrentUsage も呼ぶが、オプションはフック側が持つので query は 1 本。
  const metrics = useServiceMetrics(id);
  // CPU の実際の上界はホストのコア数(docker はコア数超えの指定でコンテナ作成を拒否する)。
  // **取れないときは客側で上界を作らない** — 知らない数字を焼くと嘘になる(CLI の cpus_to_millis
  // と同じ方針)。サーバが実コア数入りの 400 を返し、それが下の error 行に出る。
  const hostCores = metrics.data?.host_cores ?? null;
  // 親が svc 確定後にだけレンダーするので、初期化子で現値を seed できる(初期化子は
  // 再実行されない = mutation 後の refetch が編集中の値を上書きしない)。
  // **差分判定はこの seed スナップショットに対して行う**(最新の svc と比べると、polling が他所の変更を
  // 取り込んだ後に「触っていない欄」まで差分扱いになり、他所の変更を古い値で巻き戻す —
  // codex 審査 2026-08-13 の lost update)。
  const [seed] = useState(() => ({
    memory: String(svc.memory_mb ?? ""),
    cpus: svc.cpu_limit_millis != null ? String(svc.cpu_limit_millis / 1000) : "",
  }));
  const [memory, setMemory] = useState(seed.memory);
  const [cpus, setCpus] = useState(seed.cpus);
  const [inputErr, setInputErr] = useState<string | null>(null);

  // 未反映の**判定はサーバ**(CPU はホストのコア数で頭打ちされるので、期待値の計算規則を
  // 客側に写すと二箇所に増える)。ここは「どの値へ変えたのか」を自分の service 行から言うだけ。
  const pending = [
    metrics.data?.mem_limit_pending && svc.memory_mb != null ? `メモリ ${svc.memory_mb} MiB` : null,
    metrics.data?.cpu_limit_pending
      ? svc.cpu_limit_millis != null
        ? `CPU ${svc.cpu_limit_millis / 1000} コア`
        : "CPU 上限なし"
      : null,
  ]
    .filter(Boolean)
    .join(" / ");

  const submit = () => {
    if (setLimits.isPending) return;
    setInputErr(null);
    const body: SetLimitsInput = {};
    // 触った欄だけ送る(seed 比較)。不正値は黙って捨てず明示エラー(部分保存を
    // 「保存しました」と誤認させない — codex 審査)。
    if (memory.trim() !== seed.memory.trim()) {
      const mem = Number(memory.trim());
      if (!memory.trim() || !Number.isInteger(mem) || mem < 128 || mem > 4096) {
        setInputErr("メモリ上限は 128〜4096 の整数(MiB)で指定してください");
        return;
      }
      body.memory_mb = mem;
    }
    if (cpus.trim() !== seed.cpus.trim()) {
      if (cpus.trim() === "") {
        body.clear_cpu_limit = true;
      } else {
        const c = Number(cpus.trim());
        if (!Number.isFinite(c) || c < 0.1 || (hostCores != null && c > hostCores)) {
          setInputErr(
            hostCores != null
              ? `CPU 上限は 0.1〜${hostCores}(コア数)で指定してください(空欄 = 上限なし)`
              : "CPU 上限は 0.1 以上のコア数で指定してください(空欄 = 上限なし)",
          );
          return;
        }
        body.cpu_limit_millis = Math.round(c * 1000);
      }
    }
    if (!body.memory_mb && !body.cpu_limit_millis && !body.clear_cpu_limit) return; // 変更なし
    setLimits.mutate(body);
  };

  return (
    <section className="flex flex-col gap-3">
      <h2 className="text-lg font-bold text-foreground">リソース上限</h2>
      <CurrentUsage id={id} />
      {/* 設定値(DB)と適用値(動いているコンテナ)のズレ = 「変えたがまだデプロイしていない」。
          静的な「次のデプロイから反映されます」だけでは、今どちらの状態なのかが分からない
          (注入の needs_redeploy と同じ考え方)。 */}
      {pending && (
        <p className="text-sm font-semibold text-[#c98a2b]">
          設定は {pending} です。動いているコンテナにはまだ反映されていません(再デプロイで反映 —
          上の割合は今動いている値が分母)。
        </p>
      )}
      <p className="text-sm font-medium text-muted-foreground">
        変更は<strong>次のデプロイから</strong>反映されます(実行中のコンテナには影響しません)。CPU
        欄を空にすると上限なし(相対的な重み付けのみ)に戻ります。
      </p>
      <div className="flex flex-wrap items-end gap-3">
        <Input
          label="メモリ上限(MiB、128〜4096)"
          value={memory}
          inputMode="numeric"
          disabled={setLimits.isPending}
          onChange={(e) => setMemory(e.target.value)}
          className="w-44"
        />
        {/* 上界は固定値ではなくホストのコア数。数字を焼き込まず、取れた値を出す。
            「0.1 コア」だけでは何割なのか伝わらないので、入力に対する全体比を即時に添える。 */}
        <div className="flex flex-col gap-1">
          <Input
            label={hostCores ? `CPU 上限(コア数、0.1〜${hostCores})` : "CPU 上限(コア数)"}
            value={cpus}
            inputMode="decimal"
            placeholder="なし"
            disabled={setLimits.isPending}
            onChange={(e) => setCpus(e.target.value)}
            className="w-44"
          />
          <p className="text-xs font-medium text-muted-foreground">{cpuHint(cpus, hostCores)}</p>
        </div>
        <Button type="primary" loading={setLimits.isPending} onClick={submit}>
          保存
        </Button>
      </div>
      {inputErr && <p className="text-sm font-semibold text-[#e05a5a]">{inputErr}</p>}
      {setLimits.error && (
        <p className="text-sm font-semibold text-[#e05a5a]">{setLimits.error.message}</p>
      )}
      {setLimits.isSuccess && !setLimits.isPending && (
        <p className="text-sm font-semibold text-[#11a89b]">
          保存しました。次のデプロイから反映されます。
        </p>
      )}
    </section>
  );
}
