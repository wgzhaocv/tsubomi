import { useState } from "react";
import { ArrowUpRight, Plus, X } from "lucide-react";
import { Link, useParams } from "react-router";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { Select } from "@/components/ui/select";
import { useDatabases } from "@/lib/databases";
import {
  type Injection,
  useCreateInjection,
  useEjectInjection,
  useServiceInjections,
} from "@/lib/services";
import { useVolumes } from "@/lib/volumes";

// 注入元リソースの詳細ページ(クリックで遷移)。種別でルートが分かれる。
function resourceHref(inj: Injection): string {
  return inj.resource_kind === "database"
    ? `/databases/${inj.resource_id}`
    : `/volumes/${inj.resource_id}`;
}

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
        データベースやボリューム(フォルダ)を、環境変数を通じてこのサービスに注入します。接続情報やマウント先が環境変数として渡されます。
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
              <div className="flex min-w-0 flex-col gap-1">
                {/* 1 行目:環境変数名(これがコンテナに渡る名前)。 */}
                <code className="font-mono text-sm font-bold text-foreground">{inj.env_var}</code>
                {/* 2 行目:注入元のリソース(クリックで該当リソースへ)+ 値の説明。
                    volume はマウント先パス、database は接続文字列(値は表示しない)。 */}
                <span className="flex flex-wrap items-center gap-x-1.5 gap-y-0.5 text-xs font-medium text-muted-foreground">
                  <span>{inj.resource_kind}</span>
                  {inj.valid ? (
                    <Link
                      to={resourceHref(inj)}
                      className="inline-flex items-center gap-0.5 font-bold text-[#11a89b] underline-offset-2 hover:underline"
                    >
                      「{inj.resource_name}」
                      <ArrowUpRight className="size-3 shrink-0" />
                    </Link>
                  ) : (
                    <span className="font-bold text-foreground">「{inj.resource_name}」</span>
                  )}
                  {inj.resource_kind === "volume" ? (
                    inj.mount_path && <span>→ {inj.mount_path} にマウント</span>
                  ) : (
                    <span>の接続文字列</span>
                  )}
                  {!inj.valid && (
                    <span className="rounded-full bg-[#e05a5a]/15 px-2 py-0.5 font-bold text-[#e05a5a]">
                      失効(注入元が削除済み)
                    </span>
                  )}
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
        {/* form で囲み Enter 送信を効かせる。フッターの注入ボタンは Modal の外側領域に
            あるため、複数フィールドでも暗黙送信が効くよう隠し submit を 1 つ置く。 */}
        <form
          onSubmit={(e) => {
            e.preventDefault();
            submit();
          }}
          className="flex w-full flex-col gap-3"
        >
          <button type="submit" className="hidden" aria-hidden tabIndex={-1} />
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
        </form>
      </Modal>
    </div>
  );
}
