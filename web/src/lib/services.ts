import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

// service リソースのサーバ状態。databases.ts / volumes.ts と同じ作法:生の fetch +
// それを包む TanStack Query フック。一覧は Query が単一の真実源。
//
// service は GitHub repo と 1:1 のデプロイ単位。create のレスポンスだけが deploy_key /
// registry pass の **平文**を返す(以後 API では出さない)。平台は GitHub に触れないので、
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
  // 公開範囲:private(route 無し = 公網不可視)/ company(既定 = 会社 IP のみ)/
  // public(全網)。旧サーバ相手では欠ける = company 扱い。
  visibility?: string;
  // true = 有状態(deploy は stop-first:数秒瞬断・データ目録の単独占有。自帯 DB 等)。
  // 旧サーバ相手では欠ける = false 扱い。
  stateful?: boolean;
  // メモリ硬上限 MiB。旧サーバ相手では欠ける。
  memory_mb?: number;
};

export type RegistryCreds = { host: string; user: string; pass: string };

// POST /api/services のレスポンス(ServiceDto をフラット展開 + 連携用の値)。
export type CreateServiceResult = Service & {
  deploy_key: string;
  registry: RegistryCreds;
  hook_url: string;
  platforms: string;
  workflow_yaml: string;
  // GitHub 連携の手順コマンド列。平台が単一真源として組み立てる(web は表示するだけ)。
  setup_commands: string[];
};

// 公開範囲の実効値。旧サーバのフィールド欠落・未知値はどちらも company へ倒す(サーバ側
// Visibility::from_db と同じ防御方針。未知値を直通させると Radio がどの選択肢にも一致せず
// 空選択で描画される)。wire 契約のフォールバックはここが単一真源。
export function serviceVisibility(svc?: Pick<Service, "visibility">): string {
  const v = svc?.visibility;
  return v === "private" || v === "public" ? v : "company";
}

// 公開範囲の選択肢(値 + 日本語ラベル)。詳細ページ(Radio)と作成フォーム(Select)が
// 共有する単一真源 — 値・文言のドリフトを防ぐ。値はサーバの Visibility と対。
export const VISIBILITY_OPTIONS = [
  { value: "private", label: "非公開(外部からアクセス不可)" },
  { value: "company", label: "社内のみ(会社 IP のみ)" },
  { value: "public", label: "一般公開(IP 制限なし)" },
] as const;

// `sha256:<64hex>` → `sha256:<先頭 12>`(表示用の短縮)。Overview / Deploys で共用。
export function shortDigest(d: string): string {
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

export function useCreateInjection(id: string) {
  return useServiceAction<{ resource_id: string; env_var?: string; mount_path?: string }>(
    (req) => ({ url: `/api/services/${id}/injections`, method: "POST", body: req }),
    () => serviceKeys.injections(id),
  );
}

export function useEjectInjection(id: string) {
  return useServiceAction<string>(
    (injectionId) => ({ url: `/api/injections/${injectionId}`, method: "DELETE" }),
    () => serviceKeys.injections(id),
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
