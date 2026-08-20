import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

// service リソースのサーバ状態。databases.ts / volumes.ts と同じ作法:生の fetch +
// それを包む TanStack Query フック。一覧は Query が単一の真実源。
//
// service は GitHub repo と 1:1 のデプロイ単位。create のレスポンスだけが deploy_key /
// registry pass の **平文**を返す(以後 API では出さない)。プラットフォームは GitHub に触れないので、
// web は create 後にその値と「次の一手」(gh / git コマンド + workflow)を表示するだけ。

export type Service = {
  id: string;
  display_name: string;
  anon_seq: number;
  created_at: string;
  subdomain: string;
  // 観測された段階:created / deploying / running / stopped / failed。
  phase: string;
  // 期望状態:running / stopped。
  desired_state: string;
  container_port: number;
  image_digest: string | null;
  last_deploy_at: string | null;
  // 公開 URL(`<scheme>://<subdomain>.<domain>`)。サーバが算出して返す。
  // 古いサーバ相手では欠ける可能性があるので任意扱い。
  url?: string;
  // 公開範囲:private(route 無し = インターネット不可視)/ company(既定 = 会社 IP のみ)/
  // public(全網)。旧サーバ相手では欠ける = company 扱い。
  visibility?: string;
  // true = ステートフル(deploy は stop-first:数秒瞬断・データディレクトリの単独占有。持ち込み DB 等)。
  // 旧サーバ相手では欠ける = false 扱い。
  stateful?: boolean;
  // メモリ上限 MiB。旧サーバ相手では欠ける。
  memory_mb?: number;
  // CPU 上限(millicores、1000 = 1 CPU)。null / 欠け = 上限なし(相対的な重み付けのみ)。
  cpu_limit_millis?: number | null;
};

export type RegistryCreds = { host: string; user: string; pass: string };

// POST /api/services のレスポンス(ServiceDto をフラット展開 + 連携用の値)。
export type CreateServiceResult = Service & {
  deploy_key: string;
  registry: RegistryCreds;
  hook_url: string;
  platforms: string;
  workflow_yaml: string;
  // GitHub 連携の手順コマンド列。プラットフォームが単一真源として組み立てる(web は表示するだけ)。
  setup_commands: string[];
};

// 公開範囲の実効値。旧サーバのフィールド欠落・未知値はどちらも company へ倒す(サーバ側
// Visibility::from_db と同じ防御方針。未知値を直通させると Radio がどの選択肢にも一致せず
// 空選択で描画される)。wire 契約のフォールバックはここが単一真源。
export function serviceVisibility(svc?: Pick<Service, "visibility">): string {
  const v = svc?.visibility;
  return v === "private" || v === "public" ? v : "company";
}

// 公開範囲の選択肢(値 + 日本語ラベル)。詳細ページ(Radio)/ 作成フォーム(Select)/
// 一覧カードのチップが共有する単一真源 — 値・文言のドリフトを防ぐ。値はサーバの Visibility と対。
// label は short + detail から合成する(チップは short だけ使う — 文言を直しても割れない)。
export const VISIBILITY_OPTIONS = (
  [
    { value: "private", short: "非公開", detail: "外部からアクセス不可" },
    { value: "company", short: "社内のみ", detail: "会社 IP のみ" },
    { value: "public", short: "一般公開", detail: "IP 制限なし" },
  ] as const
).map((o) => ({ ...o, label: `${o.short}(${o.detail})` }));

// `sha256:<64hex>` → `sha256:<先頭 12>`(表示用の短縮)。Overview / Deploys で共用。
export function shortDigest(d: string): string {
  // deploy-source の取得中プレースホルダ('pending')は digest ではないので分かる文言にする。
  if (d === "pending") return "取得中…";
  const i = d.indexOf(":");
  return i >= 0 ? `${d.slice(0, i + 1)}${d.slice(i + 1, i + 13)}` : d.slice(0, 19);
}

// 状態の日本語ラベル(画面表示用)。wire 値(英語の enum)はそのまま色分け等に使い、
// 表示だけ日本語にする。未知の値はそのまま出す(前方互換)。
const PHASE_LABEL: Record<string, string> = {
  created: "作成済み",
  deploying: "デプロイ中",
  running: "稼働中",
  stopped: "停止中",
  failed: "失敗",
};
const DESIRED_LABEL: Record<string, string> = { running: "稼働", stopped: "停止" };
const DEPLOY_STATUS_LABEL: Record<string, string> = {
  received: "受付",
  pulling: "取得中",
  deploying: "デプロイ中",
  starting: "起動中",
  succeeded: "成功",
  failed: "失敗",
};

// service の観測段階(phase)。
export function phaseLabel(phase: string): string {
  return PHASE_LABEL[phase] ?? phase;
}
// 期望状態(desired_state)。
export function desiredLabel(state: string): string {
  return DESIRED_LABEL[state] ?? state;
}
// デプロイ status。
export function deployStatusLabel(status: string): string {
  return DEPLOY_STATUS_LABEL[status] ?? status;
}
// デプロイの契機(provenance)。**user は空文字**を返す — 大半の行がそれなので、
// 出すと全行に同じラベルが並んで情報量がゼロになる。未知値も空(前方互換)。
const DEPLOY_TRIGGER_LABEL: Record<string, string> = {
  reconcile: "自動:復活",
  caller_relink: "自動:注入元の改名に追従",
};
export function deployTriggerLabel(trigger?: string): string {
  return (trigger && DEPLOY_TRIGGER_LABEL[trigger]) || "";
}

// deploys 履歴の 1 行(DeployDto 鏡)。
export type Deploy = {
  id: string;
  git_sha: string;
  // commit の件名(旧 deploy / 旧 workflow は null → git_sha に回退)。
  commit_message: string | null;
  image_digest: string;
  status: string;
  error: string | null;
  created_at: string;
  finished_at: string | null;
  /** 契機:user / reconcile / caller_relink(旧サーバは欠ける)。平台が自動で起こした行を
   *  ユーザ自身の再デプロイと区別するための provenance。 */
  trigger?: string;
};

// 稼働中コンテナの 1 発メトリクス(ServiceMetricsDto 鏡)。停止中でも 200 で
// running:false が返る。取得できなかった項目は null(サーバが docker から拾えない場合)。
export type ServiceMetrics = {
  running: boolean;
  cpu_pct?: number | null;
  mem_bytes?: number | null;
  mem_limit_bytes?: number | null;
  restart_count?: number | null;
  started_at?: string | null;
  oom_killed?: boolean | null;
  // ホストの CPU コア数。cpu_pct は docker 由来で 100% = 1 コアなので、これが無いと
  // 「多い / 少ない」が言えない。個別上限が無いときの百分率の分母 + 上限入力の上界。
  host_cores?: number | null;
  // **今のコンテナに適用されている** CPU 上限(millicores)。null = 上限なし。DB の設定値
  // (`Service.cpu_limit_millis` = 次のデプロイ用の期望値)とは別物。
  cpu_limit_millis?: number | null;
  // 設定値がまだコンテナに反映されていないか(判定はサーバ — CPU は頭打ち規則が絡む)。
  mem_limit_pending?: boolean | null;
  cpu_limit_pending?: boolean | null;
};

// GET /api/services/:id/stats(アクセス統計)。shared の ServiceStatsDto と対。
// 口径:requests は全リクエスト(静的資産・API 込み)、visitors は bot 除外・
// 日単位リセットの匿名 visitor id の distinct(サーバ側コメント参照)。
export type StatsSlice = { key: string; requests: number };
export type ServiceStats = {
  days: number;
  interval: "hour" | "day";
  // 集計窓(interval 境界へ切り下げ済み・UTC)。チャートの 0 埋めはこの範囲で行う。
  from: string;
  to: string;
  series: { t: string; requests: number; visitors: number }[];
  totals: {
    requests: number;
    visitors: number;
    bot_requests: number;
    avg_duration_ms: number | null;
  };
  top_paths: StatsSlice[];
  statuses: StatsSlice[];
  devices: StatsSlice[];
  browsers: StatsSlice[];
  oses: StatsSlice[];
  countries: StatsSlice[];
  referers: StatsSlice[];
};

// 注入のバインディング(InjectionDto 鏡)。valid=false は失効(注入元が削除済み)。
export type Injection = {
  id: string;
  resource_id: string;
  resource_kind: string;
  resource_name: string;
  env_var: string;
  mount_path: string | null;
  valid: boolean;
  /** 作成時のみ:同名の静的 env が注入で上書きされる等の非破壊の注意喚起。 */
  warning?: string | null;
  /** 今動いているコンテナにまだ反映されていない(= 再デプロイが要る)。 */
  needs_redeploy?: boolean;
};

// この service を注入している別の service(ServiceCallerDto 鏡)= 改名の影響範囲。
// 行は caller 単位に集約済み(env_vars が複数になる)。
export type ServiceCaller = {
  id: string;
  display_name: string;
  /** 注入名(バインディング名)。派生する _HOST / _PORT は含まない。 */
  env_vars: string[];
  desired_state: string;
  last_deploy_status?: string | null;
  last_deploy_error?: string | null;
  stateful?: boolean;
  /** 連帯再デプロイが実際に動かすか。**サーバの純関数の出力をそのまま読む** —
   *  desired_state 等から再導出すると実行側の判定と食い違う。 */
  will_redeploy?: boolean;
  /** 対象外の理由(will_redeploy=false のときだけ)。 */
  skip_reason?: string | null;
};

// detail(id) = ["services", id] は deploys/injections/env/logs の prefix なので、
// detail(id) を無効化するとその service の全 tab が取り直される(prefix マッチ)。
export const serviceKeys = {
  all: ["services"] as const,
  detail: (id: string) => ["services", id] as const,
  deploys: (id: string) => ["services", id, "deploys"] as const,
  injections: (id: string) => ["services", id, "injections"] as const,
  env: (id: string) => ["services", id, "env"] as const,
  logs: (id: string) => ["services", id, "logs"] as const,
  metrics: (id: string) => ["services", id, "metrics"] as const,
  stats: (id: string, days: number) => ["services", id, "stats", days] as const,
  callers: (id: string) => ["services", id, "callers"] as const,
};

// エラー本文(サーバは AppError の日本語メッセージを text で返す)を投げる。
async function failBody(res: Response): Promise<never> {
  const body = await res.text().catch(() => "");
  throw new Error(body || `HTTP ${res.status}`);
}

export function useServices() {
  return useQuery({
    queryKey: serviceKeys.all,
    queryFn: () => getJson<Service[]>("/api/services"),
  });
}

// POST /api/services の入力。name 以外は任意 — 省略時の既定(port 8080 /
// visibility は port から推導 / stateful false / memory 1024)はサーバが単一真源として決める。
export type CreateServiceInput = {
  name: string;
  container_port?: number;
  visibility?: string;
  stateful?: boolean;
  memory_mb?: number;
  subdomain?: string;
};

export function useCreateService() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (input: CreateServiceInput): Promise<CreateServiceResult> => {
      const res = await fetch("/api/services", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(input),
      });
      if (!res.ok) return failBody(res);
      return (await res.json()) as CreateServiceResult;
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: serviceKeys.all }),
  });
}

// subdomain(= 公開 URL)の変更。旧 URL は即失効、GitHub repo 名は不変。
// この service を注入している呼び出し側は再デプロイで新値が入る(未反映バッジは注入一覧に出る)。
// 一覧 + 詳細を無効化(URL バナー / サブドメイン Stat が新値に更新される)。
export function useSetSubdomain(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (subdomain: string): Promise<Service> => {
      const res = await fetch(`/api/services/${id}/subdomain`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ subdomain }),
      });
      if (!res.ok) return failBody(res);
      return (await res.json()) as Service;
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: serviceKeys.all }),
  });
}

// リネーム(表示名のみ。subdomain = 公開 URL / GitHub repo は不変 — subdomain の変更は
// useSetSubdomain の別端点)。一覧 + 詳細を無効化。
export function useRenameService(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (name: string): Promise<Service> => {
      const res = await fetch(`/api/services/${id}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name }),
      });
      if (!res.ok) return failBody(res);
      return (await res.json()) as Service;
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: serviceKeys.all }),
  });
}

// ===== 詳細ページ(S7b)=====

// GET して JSON を返す小ヘルパ(エラー本文は failBody で投げる)。詳細の各 query が使う。
async function getJson<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) return failBody(res);
  return (await res.json()) as T;
}

// phase が遷移中(deploying)の間だけ自動更新する。reconcile(S8)が無い今は
// web からの操作 / 外部 hook の進行を画面に反映する唯一の手段がこの polling。
export function useService(id: string) {
  return useQuery({
    queryKey: serviceKeys.detail(id),
    queryFn: () => getJson<Service>(`/api/services/${id}`),
    refetchInterval: (q) => (q.state.data?.phase === "deploying" ? 4000 : false),
  });
}

// 進行中(succeeded/failed 以外)のデプロイがある間だけ自動更新。
export function useServiceDeploys(id: string) {
  return useQuery({
    queryKey: serviceKeys.deploys(id),
    queryFn: () => getJson<Deploy[]>(`/api/services/${id}/deploys`),
    refetchInterval: (q) =>
      q.state.data?.some((d) => d.status !== "succeeded" && d.status !== "failed") ? 4000 : false,
  });
}

export function useServiceInjections(id: string) {
  return useQuery({
    queryKey: serviceKeys.injections(id),
    queryFn: () => getJson<Injection[]>(`/api/services/${id}/injections`),
  });
}

// 「誰が私を注入しているか」= 改名の影響範囲。概要の常設セクションと subdomain 変更 modal が
// 同じ配列を読む。輪詢はしない(集合が変わるのは inject / eject のときで、そちらが
// serviceKeys.all を落とす)が、**呼び出し側は「未知」と「0 件」を区別しなければならない** —
// 取得前 / 失敗を空配列扱いすると、改名 modal が警告なしで通ってしまう(codex 審査)。
export function useServiceCallers(id: string) {
  return useQuery({
    queryKey: serviceKeys.callers(id),
    queryFn: () => getJson<ServiceCaller[]>(`/api/services/${id}/callers`),
  });
}

// 連帯再デプロイ(202 即返し)。応答は要求時点の計画で約束ではないので、成功後は
// callers を落として「直近デプロイの状態」を取り直させる。
export type RedeployCallersPlan = {
  /** 呼び出し側の全件。対象かどうかは各要素の will_redeploy。 */
  planned: ServiceCaller[];
};
export function useRedeployCallers(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (): Promise<RedeployCallersPlan> => {
      const res = await fetch(`/api/services/${id}/redeploy-callers`, { method: "POST" });
      if (!res.ok) return failBody(res);
      return (await res.json()) as RedeployCallersPlan;
    },
    // **callers だけを落とす**。このページには `useServiceMetrics` が居て、それは 1〜2 秒の
    // docker stats を叩く(hook のコメント参照)— 「別の service を再デプロイした」ことと
    // このサービスの CPU / メモリは無関係なので、serviceKeys.all で巻き込むと改名 1 回で
    // 香橙派の docker daemon を数秒無駄に回す(審査で実測された)。
    onSuccess: () => qc.invalidateQueries({ queryKey: serviceKeys.callers(id) }),
  });
}

export function useServiceEnvKeys(id: string) {
  return useQuery({
    queryKey: serviceKeys.env(id),
    queryFn: () => getJson<string[]>(`/api/services/${id}/env`),
  });
}

// ログは tab 表示中だけ自動更新(5 秒ごと)。tail はサーバ既定(200)。
// poll=false(コンテナが走っていない)なら初回取得のみで polling しない(空応答の無駄打ちを避ける)。
export function useServiceLogs(id: string, poll = true) {
  return useQuery({
    queryKey: serviceKeys.logs(id),
    queryFn: () => getJson<{ logs: string }>(`/api/services/${id}/logs`),
    refetchInterval: poll ? 5000 : false,
  });
}

// 稼働中コンテナの使用量。**上限を決めるための材料**なので概要ページで上限の隣に出す。
// 20 秒間隔:サーバ側は CPU% を出すのに docker stats を 2 サンプル取る(1 回 1〜2 秒、
// 共有ホストの Pi)ので、ログ(5 秒)より粗くする。画面を離れている間は取りに行かない
// (TanStack の既定 = background では refetchInterval が止まる)。取得失敗は
// 補助情報なので黙って諦める(概要の他の部分を赤字で汚さない)。
// 追従の判断をフック内に閉じる:止まっているコンテナの使用量は変化しないので輪詢しない。
// **呼び出し側にオプションを持たせない**のが要点 — 同じ query key を別オプションで複数箇所から
// 呼ぶと、輪詢するかどうかが「最後にマウントされた側」に依存して揺れる。判断が中にあれば、
// どの画面から何箇所呼んでも 1 本の query・1 本のタイマーに収束する。
export function useServiceMetrics(id: string) {
  return useQuery({
    queryKey: serviceKeys.metrics(id),
    queryFn: () => getJson<ServiceMetrics>(`/api/services/${id}/metrics`),
    refetchInterval: (q) => (q.state.data?.running ? 20_000 : false),
    retry: false,
  });
}

// アクセス統計。days はサーバ側の集計パラメータなので query key に含める(期間切替 = 別 query)。
// 過去分は不変・新着も分単位で困らないので polling しない(staleTime 既定 60s)。
export function useServiceStats(id: string, days: number) {
  return useQuery({
    queryKey: serviceKeys.stats(id, days),
    queryFn: () => getJson<ServiceStats>(`/api/services/${id}/stats?days=${days}`),
  });
}

// POST/DELETE して(任意 body)、成功後に指定 key を無効化する共通 mutation 生成。
// id は build / invalidate の各クロージャが呼び出し側のフックから捕捉するのでここでは取らない。
function useServiceAction<V>(
  build: (v: V) => { url: string; method: string; body?: unknown },
  invalidate: () => readonly unknown[],
) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (v: V): Promise<void> => {
      const { url, method, body } = build(v);
      const res = await fetch(url, {
        method,
        headers: body !== undefined ? { "Content-Type": "application/json" } : undefined,
        body: body !== undefined ? JSON.stringify(body) : undefined,
      });
      if (!res.ok) return failBody(res);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: invalidate() }),
  });
}

// lifecycle:phase/desired/deploys が変わるので service 全体(all = 一覧 + 全詳細 tab)を無効化。
export function useStartService(id: string) {
  return useServiceAction<void>(
    () => ({ url: `/api/services/${id}/start`, method: "POST" }),
    () => serviceKeys.all,
  );
}

export function useStopService(id: string) {
  return useServiceAction<void>(
    () => ({ url: `/api/services/${id}/stop`, method: "POST" }),
    () => serviceKeys.all,
  );
}

export function useDeleteService(id: string) {
  return useServiceAction<void>(
    () => ({ url: `/api/services/${id}`, method: "DELETE" }),
    () => serviceKeys.all,
  );
}

export function useRollbackService(id: string) {
  return useServiceAction<string>(
    (deployId) => ({
      url: `/api/services/${id}/rollback`,
      method: "POST",
      body: { deploy_id: deployId },
    }),
    () => serviceKeys.all,
  );
}

// 公開範囲の切替(即時反映・再デプロイ不要)。値は private / company / public。
export function useSetServiceVisibility(id: string) {
  return useServiceAction<string>(
    (visibility) => ({
      url: `/api/services/${id}/visibility`,
      method: "POST",
      body: { visibility },
    }),
    () => serviceKeys.all,
  );
}

// memory / cpus 上限の変更。**次のデプロイから反映**(実行中のコンテナには影響しない)。
export type SetLimitsInput = {
  memory_mb?: number;
  cpu_limit_millis?: number;
  clear_cpu_limit?: boolean;
};
export function useSetServiceLimits(id: string) {
  return useServiceAction<SetLimitsInput>(
    (body) => ({ url: `/api/services/${id}/limits`, method: "POST", body }),
    () => serviceKeys.all,
  );
}

// 他の mutation と違い**応答 body を返す** — server が `warning`(静的 env に譲った派生 env 等)を
// 載せてくるので、黙って捨てると web 利用者だけが気付けない。
export function useCreateInjection(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (req: {
      resource_id: string;
      env_var?: string;
      mount_path?: string;
    }): Promise<Injection> => {
      const res = await fetch(`/api/services/${id}/injections`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(req),
      });
      if (!res.ok) return failBody(res);
      return res.json();
    },
    // **serviceKeys.all で広く落とす**:注入の作成は自分の injections だけでなく
    // **注入元(callee)の逆引き名単**(serviceKeys.callers)も変える。狭いキーだけ落とすと、
    // callee の概要が古い `[]` を最大 staleTime 分そのまま使い、改名時に「呼び出し側なし」と
    // 嘘をつく(codex 審査)。注入は稀な操作なので広く落とす方の代償は小さい。
    onSuccess: () => qc.invalidateQueries({ queryKey: serviceKeys.all }),
  });
}

// eject は injection id だけで引ける(端点が `/api/injections/{id}`)ので service id を取らない。
export function useEjectInjection() {
  return useServiceAction<string>(
    (injectionId) => ({ url: `/api/injections/${injectionId}`, method: "DELETE" }),
    // 作成と同じ理由で広く落とす(注入元の逆引き名単も変わる — codex 審査)。
    () => serviceKeys.all,
  );
}

export function useSetEnv(id: string) {
  return useServiceAction<{ key: string; value: string }>(
    (req) => ({ url: `/api/services/${id}/env`, method: "POST", body: req }),
    () => serviceKeys.env(id),
  );
}

export function useUnsetEnv(id: string) {
  return useServiceAction<string>(
    (key) => ({
      url: `/api/services/${id}/env/${encodeURIComponent(key)}`,
      method: "DELETE",
    }),
    () => serviceKeys.env(id),
  );
}
