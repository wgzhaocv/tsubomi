import { useState } from "react";
import { Plus, X } from "lucide-react";
import { useParams } from "react-router";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { useServiceEnvKeys, useSetEnv, useUnsetEnv } from "@/lib/services";

// 環境変数:key だけを一覧表示する(値は秘密として出さない=上書きのみ)。
// 値はコンテナ起動の瞬間に解決されるので、反映には再デプロイ(または開始)が要る。
export default function ServiceEnv() {
  const { id = "" } = useParams();
  const { data: keys, isPending, error } = useServiceEnvKeys(id);
  const setEnv = useSetEnv(id);
  const unset = useUnsetEnv(id);

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
        コンテナに渡す環境変数です。値は表示されません(上書きのみ)。
        <strong>反映には再デプロイ(または開始)が必要</strong>です。
      </p>

      {error && <p className="text-sm font-semibold text-[#e05a5a]">{error.message}</p>}

      {!isPending && keys && keys.length === 0 && (
        <p className="text-sm font-medium text-muted-foreground">(まだ環境変数はありません)</p>
      )}
      {keys && keys.length > 0 && (
        <ul className="flex flex-col gap-2">
          {keys.map((k) => (
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
