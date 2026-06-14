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

export const serviceKeys = {
  all: ["services"] as const,
};

// エラー本文(サーバは AppError の日本語メッセージを text で返す)を投げる。
async function failBody(res: Response): Promise<never> {
  const body = await res.text().catch(() => "");
  throw new Error(body || `HTTP ${res.status}`);
}

async function fetchServices(): Promise<Service[]> {
  const res = await fetch("/api/services");
  if (!res.ok) return failBody(res);
  return (await res.json()) as Service[];
}

export function useServices() {
  return useQuery({ queryKey: serviceKeys.all, queryFn: fetchServices });
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
