import { Terminal, useTerminal } from "@wterm/react";
import "@wterm/react/css";
import { RotateCw } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useParams } from "react-router";

import { Button } from "@/components/ui/button";
import { useService } from "@/lib/services";

// コンテナ内の **対話シェル**(/bin/sh)。所有者が自分の稼働中コンテナへブラウザから入る
// (web 専用 — 対話 PTY は CLI の AI フレンドリ JSON 契約に合わない。CLI は一発 `tbm service exec`)。
// 暴露レベルは web SQL と同一ティア(env 注入値が見える等は受容済み)。
//
// ワイヤープロトコル(後端 docker::handle_terminal と対):
//   client→server  Binary=生 stdin / Text(JSON)=制御 `{"type":"resize","cols","rows"}`
//   server→client  Binary=exec 出力(失敗通知も人間可読の Binary)
// 稼働中(phase==="running")のときだけ端末を mount = WS を開く。それ以外は案内のみ
// (後端でも ensure_owned + 稼働中を二重に検証)。

export default function ServiceTerminal() {
  const { id = "" } = useParams();
  const { data: svc, isPending } = useService(id);

  return (
    <div className="flex flex-col gap-4">
      <h2 className="text-lg font-bold text-foreground">ターミナル</h2>
      <p className="text-sm font-medium text-muted-foreground">
        稼働中コンテナ内の対話シェル(/bin/sh)です。env や ps、curl などで内部状態を確認できます。
      </p>

      {isPending ? (
        <p className="text-sm font-medium text-muted-foreground">読み込み中…</p>
      ) : svc?.phase === "running" ? (
        // key で「再接続」時に確実に作り直す(WS と端末を新規に張り直す)。
        <TerminalPane id={id} />
      ) : (
        <p className="text-sm font-medium text-muted-foreground">
          コンテナが走っていません。先にデプロイして running にしてから開いてください。
        </p>
      )}
    </div>
  );
}

type ConnState = "connecting" | "open" | "closed";

// 1 セッション。WS を張り、端末の入力(onData)を Binary で送り、サーバからの Binary を端末へ書く。
// resize は Text(JSON)で送る。unmount(タブ離脱)で WS を閉じる = 後端で sh が終了する。
function TerminalPane({ id }: { id: string }) {
  const { ref, write } = useTerminal();
  const wsRef = useRef<WebSocket | null>(null);
  // write は再レンダリングで作り直され得るので ref 越しに最新を呼ぶ(WS を貼り直さないため、
  // effect の依存は id と再接続トークンだけにする)。
  const writeRef = useRef(write);
  writeRef.current = write;
  const enc = useRef(new TextEncoder());
  const [state, setState] = useState<ConnState>("connecting");
  // 「再接続」ボタンで増やすと effect が貼り直る。
  const [nonce, setNonce] = useState(0);

  useEffect(() => {
    const scheme = location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${scheme}://${location.host}/api/services/${id}/terminal`);
    ws.binaryType = "arraybuffer";
    wsRef.current = ws;
    setState("connecting");

    // 再接続時、古い socket の遅延 onclose/onmessage が新 socket を上書きしないよう
    // 毎ハンドラで「自分が現役か」を確認する(cleanup で wsRef を null/新値にしてから閉じる)。
    ws.onopen = () => {
      if (wsRef.current === ws) setState("open");
    };
    ws.onmessage = (ev) => {
      if (wsRef.current !== ws) return;
      // 出力は Binary(失敗通知も人間可読バイト)。互換のため string も書ける。
      if (ev.data instanceof ArrayBuffer) writeRef.current(new Uint8Array(ev.data));
      else if (typeof ev.data === "string") writeRef.current(ev.data);
    };
    ws.onclose = () => {
      if (wsRef.current === ws) setState("closed");
    };

    // unmount で必ず閉じる(後端は input drop → stdin EOF → sh 終了 = ゾンビを残さない)。
    return () => {
      wsRef.current = null;
      ws.close();
    };
  }, [id, nonce]);

  const send = (data: string | BufferSource) => {
    const ws = wsRef.current;
    if (ws && ws.readyState === WebSocket.OPEN) ws.send(data);
  };

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between gap-3">
        <span className="text-xs font-semibold text-muted-foreground">
          {state === "open"
            ? "接続中"
            : state === "connecting"
              ? "接続しています…"
              : "切断されました(シェル終了 / タイムアウト)"}
        </span>
        {state === "closed" && (
          <Button
            type="default"
            size="small"
            icon={<RotateCw className="size-4" />}
            onClick={() => setNonce((n) => n + 1)}
          >
            再接続
          </Button>
        )}
      </div>
      <Terminal
        ref={ref}
        autoResize
        cursorBlink
        className="h-[480px] overflow-hidden rounded-2xl border-2 border-[#e8e2d6] p-2"
        onData={(d) => send(enc.current.encode(d))}
        onResize={(cols, rows) => send(JSON.stringify({ type: "resize", cols, rows }))}
      />
    </div>
  );
}
