import { useState } from "react";
import { Plus, X } from "lucide-react";
import { useParams } from "react-router";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { Select } from "@/components/ui/select";
import { useDatabases } from "@/lib/databases";
import { useCreateInjection, useEjectInjection, useServiceInjections } from "@/lib/services";
import { useVolumes } from "@/lib/volumes";

// 注入:database / volume を service にバインドする。値はコンテナ起動の瞬間に解決されるので
// 反映には再デプロイが要る。失効(注入元が削除済み)は valid:false でバッジ表示。
export default function ServiceInjections() {
  const { id = "" } = useParams();
  const { data: injections, error } = useServiceInjections(id);
  const { data: dbs } = useDatabases();
  const { data: vols } = useVolumes();
  const create = useCreateInjection(id);
  const eject = useEjectInjection(id);

  const [open, setOpen] = useState(false);
  const [resourceId, setResourceId] = useState("");
  const [envVar, setEnvVar] = useState("");
  const [mount, setMount] = useState("");

  const options = [
    ...(dbs ?? []).map((d) => ({ key: d.id, label: `${d.display_name}(database)` })),
    ...(vols ?? []).map((v) => ({ key: v.id, label: `${v.display_name}(volume)` })),
  ];
  const isVolume = (vols ?? []).some((v) => v.id === resourceId);

  const submit = () => {
    if (!resourceId || create.isPending) return;
    create.mutate(
      {
        resource_id: resourceId,
        env_var: envVar.trim() || undefined,
        mount_path: isVolume ? mount.trim() || undefined : undefined,
      },
      {
        onSuccess: () => {
          setOpen(false);
          setResourceId("");
          setEnvVar("");
          setMount("");
        },
      },
    );
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-lg font-bold text-foreground">注入</h2>
        <Button
          type="default"
          size="small"
          icon={<Plus className="size-4" />}
          onClick={() => setOpen(true)}
        >
          リソースを注入
        </Button>
      </div>
      <p className="text-sm font-medium text-muted-foreground">
        database / volume をこのサービスに注入します。
        <strong>反映には再デプロイ(または開始)が必要</strong>です。
      </p>

      {error && <p className="text-sm font-semibold text-[#e05a5a]">{error.message}</p>}

      {injections && injections.length === 0 && (
        <p className="text-sm font-medium text-muted-foreground">(まだ注入はありません)</p>
      )}
      {injections && injections.length > 0 && (
        <ul className="flex flex-col gap-2">
          {injections.map((inj) => (
            <li
              key={inj.id}
              className="flex flex-wrap items-center justify-between gap-3 rounded-2xl border-2 border-[#e8e2d6] bg-card px-4 py-3"
            >
              <div className="flex min-w-0 flex-col gap-0.5">
                <span className="font-bold text-foreground">
                  <code className="font-mono">{inj.env_var}</code> ← {inj.resource_name}
                  {!inj.valid && (
                    <span className="ml-2 rounded-full bg-[#e05a5a]/15 px-2 py-0.5 text-xs font-bold text-[#e05a5a]">
                      失効
                    </span>
                  )}
                </span>
                <span className="text-xs font-medium text-muted-foreground">
                  {inj.resource_kind}
                  {inj.mount_path ? ` · ${inj.mount_path}` : ""}
                </span>
              </div>
              <Button
                type="text"
                size="small"
                danger
                icon={<X className="size-4" />}
                loading={eject.isPending}
                onClick={() => eject.mutate(inj.id)}
              >
                外す
              </Button>
            </li>
          ))}
        </ul>
      )}
      {eject.error && <p className="text-sm font-semibold text-[#e05a5a]">{eject.error.message}</p>}

      <Modal
        open={open}
        title="リソースを注入"
        typewriter={false}
        width={460}
        onClose={() => setOpen(false)}
        footer={
          <>
            <Button type="text" onClick={() => setOpen(false)}>
              キャンセル
            </Button>
            <Button
              type="primary"
              loading={create.isPending}
              disabled={!resourceId}
              onClick={submit}
            >
              注入
            </Button>
          </>
        }
      >
        <div className="flex w-full flex-col gap-3">
          <div className="flex flex-col gap-1.5">
            <span className="text-sm font-semibold text-foreground">リソース</span>
            <Select
              options={options}
              value={resourceId}
              onChange={setResourceId}
              placeholder="database / volume を選択"
            />
          </div>
          <Input
            label="環境変数名(任意)"
            value={envVar}
            placeholder={isVolume ? "STORAGE_PATH" : "DATABASE_URL"}
            onChange={(e) => setEnvVar(e.target.value)}
            description="省略するとリソース種別の既定名を使います。"
          />
          {isVolume && (
            <Input
              label="マウント先(任意)"
              value={mount}
              placeholder="/data/<名前>"
              onChange={(e) => setMount(e.target.value)}
            />
          )}
          {create.error && (
            <p className="text-sm font-semibold text-[#e05a5a]">{create.error.message}</p>
          )}
        </div>
      </Modal>
    </div>
  );
}
