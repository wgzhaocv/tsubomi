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

// `sha256:<64hex>` → `sha256:<先頭 12>`(表示用の短縮)。Overview / Deploys で共用。
export function shortDigest(d: string): string {
  const i = d.indexOf(":");
  return i >= 0 ? `${d.slice(0, i + 1)}${d.slice(i + 1, i + 13)}` : d.slice(0, 19);
}

// deploys 履歴の 1 行(DeployDto 鏡)。
export type Deploy = {
  id: string;
  git_sha: string;
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

export function useCreateService() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (name: string): Promise<CreateServiceResult> => {
      const res = await fetch("/api/services", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name }),
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
