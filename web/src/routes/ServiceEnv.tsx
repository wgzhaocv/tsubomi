import { useState } from "react";
import { Plus, X } from "lucide-react";
import { Link, useParams } from "react-router";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { useServiceEnvKeys, useServiceInjections, useSetEnv, useUnsetEnv } from "@/lib/services";

// 環境変数:key だけを一覧表示する(値は秘密として出さない=上書きのみ)。
// 静的な変数(ここで追加)に加えて、注入(database / volume)由来の変数も「注入」と
// 注記して一覧する(コンテナが実際に受け取る変数の全体像)。注入由来はここでは
// 削除できない(注入タブで外す)。値はコンテナ起動の瞬間に解決されるので、反映には
// 再デプロイ(または開始)が要る。
export default function ServiceEnv() {
  const { id = "" } = useParams();
  const { data: keys, isPending, error } = useServiceEnvKeys(id);
  const { data: injections } = useServiceInjections(id);
  const setEnv = useSetEnv(id);
  const unset = useUnsetEnv(id);

  // 注入由来で実際にコンテナへ渡る変数(失効した注入は値が解決されないので除く)。
  const injected = (injections ?? []).filter((i) => i.valid);

  const [open, setOpen] = useState(false);
  const [key, setKey] = useState("");
  const [value, setValue] = useState("");

  const submit = () => {
    const k = key.trim();
    if (!k || setEnv.isPending) return;
    setEnv.mutate(
      { key: k, value },
      {
        onSuccess: () => {
          setOpen(false);
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
        <Button
          type="default"
          size="small"
          icon={<Plus className="size-4" />}
          onClick={() => setOpen(true)}
        >
          変数を追加
        </Button>
      </div>
      <p className="text-sm font-medium text-muted-foreground">
        コンテナに渡す環境変数です。「注入」付きは database / volume
        の注入由来で、ここでは削除できません(
        <Link
          to="../injections"
          className="font-bold text-[#11a89b] underline-offset-2 hover:underline"
        >
          注入タブ
        </Link>
        で管理)。値は表示されません(上書きのみ)。
        <strong>反映には再デプロイ(または開始)が必要</strong>です。
      </p>

      {error && <p className="text-sm font-semibold text-[#e05a5a]">{error.message}</p>}

      {!isPending && keys && keys.length === 0 && injected.length === 0 && (
        <p className="text-sm font-medium text-muted-foreground">(まだ環境変数はありません)</p>
      )}
      {((keys && keys.length > 0) || injected.length > 0) && (
        <ul className="flex flex-col gap-2">
          {/* 注入由来(先頭・読み取り専用)。env_var が container に渡る名前。 */}
          {injected.map((inj) => (
            <li
              key={`inj-${inj.id}`}
              className="flex flex-wrap items-center justify-between gap-3 rounded-2xl border-2 border-[#e8e2d6] bg-card px-4 py-3"
            >
              <div className="flex min-w-0 items-center gap-2">
                <code className="min-w-0 truncate font-mono font-bold text-foreground">
                  {inj.env_var}
                </code>
                <span className="shrink-0 rounded-full bg-accent px-2 py-0.5 text-xs font-bold text-accent-foreground">
                  注入
                </span>
              </div>
              <Link
                to="../injections"
                className="shrink-0 text-xs font-semibold text-muted-foreground underline-offset-2 hover:text-[#11a89b] hover:underline"
              >
                {inj.resource_kind}「{inj.resource_name}」
              </Link>
            </li>
          ))}
          {/* 静的(ここで追加・削除)。 */}
          {(keys ?? []).map((k) => (
            <li
              key={k}
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
      {unset.error && <p className="text-sm font-semibold text-[#e05a5a]">{unset.error.message}</p>}

      <Modal
        open={open}
        title="環境変数を追加"
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
              loading={setEnv.isPending}
              disabled={!key.trim()}
              onClick={submit}
            >
              追加
            </Button>
          </>
        }
      >
        <form
          onSubmit={(e) => {
            e.preventDefault();
            submit();
          }}
          className="flex w-full flex-col gap-3"
        >
          {/* footer の追加ボタンは form 外なので、複数フィールドでも Enter 送信が
              効くよう隠し submit を 1 つ置く(ServiceInjections と同じ作法)。 */}
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
