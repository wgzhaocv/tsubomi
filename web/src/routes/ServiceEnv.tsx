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
  useServiceEnvKeys,
  useServiceInjections,
  useServices,
  useSetEnv,
  useUnsetEnv,
} from "@/lib/services";
import { useVolumes } from "@/lib/volumes";

// 注入元リソースの詳細ページ(クリックで遷移)。種別でルートが分かれる。
function resourceHref(inj: Injection): string {
  if (inj.resource_kind === "database") return `/databases/${inj.resource_id}`;
  if (inj.resource_kind === "service") return `/services/${inj.resource_id}`;
  return `/volumes/${inj.resource_id}`;
}

// 環境変数:この service の容器が受け取る変数を 1 画面に集約する。容器にとって注入も静的変数も
// 結局は同じ「環境」なので分けない(かつて注入は別タブだった)。
//   - 注入(database / volume 由来)は最上部に「注入」バッジ付きで特別表示し、ここで追加 / 外す。
//     volume は mount + STORAGE_PATH、database は接続文字列。失効(注入元が削除済み)は valid:false。
//   - 静的変数は key だけ一覧(値は秘密として出さない=上書きのみ)。
// 値はコンテナ起動の瞬間に解決されるので、反映には再デプロイ(または開始)が要る。
export default function ServiceEnv() {
  const { id = "" } = useParams();
  const { data: injections, error: injError } = useServiceInjections(id);
  const { data: keys, isPending, error: envError } = useServiceEnvKeys(id);
  const { data: dbs } = useDatabases();
  const { data: vols } = useVolumes();
  const { data: svcs } = useServices();
  const create = useCreateInjection(id);
  const eject = useEjectInjection(id);
  const setEnv = useSetEnv(id);
  const unset = useUnsetEnv(id);

  // 注入は失効分も含め全件出す(ここが注入の管理面なので、失効注入も「外す」で掃除できる必要がある)。
  const injs = injections ?? [];
  const staticKeys = keys ?? [];
  const isEmpty = injs.length === 0 && staticKeys.length === 0;

  // 注入 Modal のフォーム。
  const [injOpen, setInjOpen] = useState(false);
  const [resourceId, setResourceId] = useState("");
  const [injEnvVar, setInjEnvVar] = useState("");
  const [mount, setMount] = useState("");
  // 環境変数 Modal のフォーム。
  const [envOpen, setEnvOpen] = useState(false);
  const [key, setKey] = useState("");
  const [value, setValue] = useState("");

  const options = [
    ...(dbs ?? []).map((d) => ({ key: d.id, label: `${d.display_name}(database)` })),
    ...(vols ?? []).map((v) => ({ key: v.id, label: `${v.display_name}(volume)` })),
    // 別 service を注入(内部 URL)。自分自身は除く(自注入はサーバが弾く)。
    ...(svcs ?? [])
      .filter((s) => s.id !== id)
      .map((s) => ({ key: s.id, label: `${s.display_name}(service)` })),
  ];
  const isVolume = (vols ?? []).some((v) => v.id === resourceId);
  const isService = (svcs ?? []).some((s) => s.id === resourceId);

  const submitInjection = () => {
    if (!resourceId || create.isPending) return;
    create.mutate(
      {
        resource_id: resourceId,
        env_var: injEnvVar.trim() || undefined,
        mount_path: isVolume ? mount.trim() || undefined : undefined,
      },
      {
        onSuccess: () => {
          setInjOpen(false);
          setResourceId("");
          setInjEnvVar("");
          setMount("");
        },
      },
    );
  };

  const submitEnv = () => {
    const k = key.trim();
    if (!k || setEnv.isPending) return;
    setEnv.mutate(
      { key: k, value },
      {
        onSuccess: () => {
          setEnvOpen(false);
          setKey("");
          setValue("");
        },
      },
    );
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-lg font-bold text-foreground">環境変数</h2>
        <div className="flex flex-wrap items-center gap-2">
          <Button
            type="default"
            size="small"
            icon={<Plus className="size-4" />}
            onClick={() => setInjOpen(true)}
          >
            リソースを注入
          </Button>
          <Button
            type="default"
            size="small"
            icon={<Plus className="size-4" />}
            onClick={() => setEnvOpen(true)}
          >
            変数を追加
          </Button>
        </div>
      </div>
      <p className="text-sm font-medium text-muted-foreground">
        コンテナに渡す環境変数の全体像です。「注入」付きは database / volume / service
        の注入由来で、接続情報・マウント先・別 app の内部 URL
        が環境変数として渡されます(「リソースを注入」で追加・「外す」で解除)。 service 注入は URL
        に加えて <code className="font-mono">〜_HOST</code> /{" "}
        <code className="font-mono">〜_PORT</code> も渡されます(データベース等の非 HTTP
        コンテナへ自分で接続文字列を組む用)。
        その他はここで追加した静的変数です(値は表示されません=上書きのみ)。
        <strong>反映には再デプロイ(または開始)が必要</strong>です。
      </p>

      {injError && <p className="text-sm font-semibold text-[#e05a5a]">{injError.message}</p>}
      {envError && <p className="text-sm font-semibold text-[#e05a5a]">{envError.message}</p>}

      {!isPending && isEmpty && (
        <p className="text-sm font-medium text-muted-foreground">(まだ環境変数はありません)</p>
      )}
      {!isEmpty && (
        <ul className="flex flex-col gap-2">
          {/* 注入由来(先頭・特別表示)。env_var が container に渡る名前。失効分も掃除のため出す。 */}
          {injs.map((inj) => (
            <li
              key={`inj-${inj.id}`}
              className="flex flex-wrap items-center justify-between gap-3 rounded-2xl border-2 border-[#e8e2d6] bg-card px-4 py-3"
            >
              <div className="flex min-w-0 flex-col gap-1">
                {/* 1 行目:環境変数名(これがコンテナに渡る名前)+「注入」バッジ。 */}
                <div className="flex min-w-0 items-center gap-2">
                  <code className="min-w-0 truncate font-mono text-sm font-bold text-foreground">
                    {inj.env_var}
                  </code>
                  <span className="shrink-0 rounded-full bg-accent px-2 py-0.5 text-xs font-bold text-accent-foreground">
                    注入
                  </span>
                </div>
                {/* 2 行目:注入元のリソース(クリックで該当リソースへ)+ 値の説明。
                    volume はマウント先パス、service は内部 URL、その他は接続文字列(値は表示しない)。 */}
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
                  ) : inj.resource_kind === "service" ? (
                    <span>の内部 URL</span>
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
          {/* 注入と静的の境目(両方ある時だけ)。 */}
          {injs.length > 0 && staticKeys.length > 0 && (
            <li aria-hidden className="mx-1 my-1 border-t-2 border-dashed border-[#e8e2d6]" />
          )}
          {/* 静的(ここで追加・削除)。 */}
          {staticKeys.map((k) => (
            <li
              key={`env-${k}`}
              className="flex flex-wrap items-center justify-between gap-3 rounded-2xl border-2 border-[#e8e2d6] bg-card px-4 py-3"
            >
              <code className="min-w-0 truncate font-mono font-bold text-foreground">{k}</code>
              <Button
                type="text"
                size="small"
                danger
                icon={<X className="size-4" />}
                loading={unset.isPending}
                onClick={() => unset.mutate(k)}
              >
                削除
              </Button>
            </li>
          ))}
        </ul>
      )}
      {eject.error && <p className="text-sm font-semibold text-[#e05a5a]">{eject.error.message}</p>}
      {unset.error && <p className="text-sm font-semibold text-[#e05a5a]">{unset.error.message}</p>}

      {/* リソースを注入(database / volume を service にバインド)。 */}
      <Modal
        open={injOpen}
        title="リソースを注入"
        typewriter={false}
        width={460}
        onClose={() => setInjOpen(false)}
        footer={
          <>
            <Button type="text" onClick={() => setInjOpen(false)}>
              キャンセル
            </Button>
            <Button
              type="primary"
              loading={create.isPending}
              disabled={!resourceId}
              onClick={submitInjection}
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
            submitInjection();
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
              placeholder="database / volume / service を選択"
            />
          </div>
          <Input
            label="環境変数名(任意)"
            value={injEnvVar}
            placeholder={isVolume ? "STORAGE_PATH" : isService ? "<名前>_URL" : "DATABASE_URL"}
            onChange={(e) => setInjEnvVar(e.target.value)}
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

      {/* 静的な環境変数を追加。 */}
      <Modal
        open={envOpen}
        title="環境変数を追加"
        typewriter={false}
        width={460}
        onClose={() => setEnvOpen(false)}
        footer={
          <>
            <Button type="text" onClick={() => setEnvOpen(false)}>
              キャンセル
            </Button>
            <Button
              type="primary"
              loading={setEnv.isPending}
              disabled={!key.trim()}
              onClick={submitEnv}
            >
              追加
            </Button>
          </>
        }
      >
        <form
          onSubmit={(e) => {
            e.preventDefault();
            submitEnv();
          }}
          className="flex w-full flex-col gap-3"
        >
          {/* footer の追加ボタンは form 外なので、複数フィールドでも Enter 送信が
              効くよう隠し submit を 1 つ置く。 */}
          <button type="submit" className="hidden" aria-hidden tabIndex={-1} />
          <Input
            label="キー"
            value={key}
            autoFocus
            placeholder="API_KEY"
            onChange={(e) => setKey(e.target.value)}
          />
          <Input
            label="値"
            value={value}
            placeholder="(値)"
            onChange={(e) => setValue(e.target.value)}
            description="同じキーを再度追加すると上書きします。"
          />
          {setEnv.error && (
            <p className="text-sm font-semibold text-[#e05a5a]">{setEnv.error.message}</p>
          )}
        </form>
      </Modal>
    </div>
  );
}
