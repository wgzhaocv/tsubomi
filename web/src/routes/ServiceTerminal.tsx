import { Terminal, useTerminal } from "@wterm/react";
import { RotateCw } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useParams } from "react-router";

import { Button } from "@/components/ui/button";
import { useService } from "@/lib/services";

// コンテナ内の **対話シェル**(/bin/sh)。所有者が自分の稼働中コンテナへブラウザから入る
// (web 専用 — 対話 PTY は CLI の AI フレンドリ JSON 契約に合わない。CLI は一発 `tbm service exec`)。
// 暴露レベルは web SQL と同一ティア(env 注入値が見える等は受容済み)。
//
// ワイヤープロトコル(バックエンド docker::handle_terminal と対):
//   client→server  Binary=生 stdin / Text(JSON)=制御 `{"type":"resize","cols","rows"}`
//   server→client  Binary=exec 出力(失敗通知も人間可読の Binary)
// 稼働中(phase==="running")のときだけ端末を mount = WS を開く。それ以外は案内のみ
// (バックエンドでも ensure_owned + 稼働中を二重に検証)。

export default function ServiceTerminal() {
  const { id = "" } = useParams();
  const { data: svc, isPending } = useService(id);
  // 再接続 = key で TerminalPane を丸ごと作り直す(WS も端末画面も新規 = 旧セッションの
  // 画面を持ち越さない。ライブラリに clear API が無いので remount で清める)。
  const [nonce, setNonce] = useState(0);

  return (
    <div className="flex flex-col gap-4">
      <h2 className="text-lg font-bold text-foreground">ターミナル</h2>
      <p className="text-sm font-medium text-muted-foreground">
        稼働中コンテナ内の対話シェル(/bin/sh)です。env や ps、curl などで内部状態を確認できます。
      </p>

      {isPending ? (
        <p className="text-sm font-medium text-muted-foreground">読み込み中…</p>
      ) : svc?.phase === "running" ? (
        <TerminalPane key={`${id}:${nonce}`} id={id} onReconnect={() => setNonce((n) => n + 1)} />
      ) : (
        <p className="text-sm font-medium text-muted-foreground">
          コンテナが走っていません。先にデプロイして running にしてから開いてください。
        </p>
      )}
    </div>
  );
}

type ConnState = "connecting" | "open" | "closed";

const enc = new TextEncoder();

// 1 セッション。WS を張り、端末の入力(onData)を Binary で送り、サーバからの Binary を端末へ書く。
// resize は Text(JSON)で送る。unmount(タブ離脱 / 親の key 更新)で WS を閉じる
// = バックエンドで sh が終了する。再接続は親が key を変えて丸ごと作り直す。
function TerminalPane({ id, onReconnect }: { id: string; onReconnect: () => void }) {
  // write/focus は useCallback([]) の恒等安定(呼び出し時に ref 経由で実体へ届く)なので、
  // effect の依存に入れても貼り直しは起きない。
  const { ref, write, focus } = useTerminal();
  const wsRef = useRef<WebSocket | null>(null);
  const [state, setState] = useState<ConnState>("connecting");
  // WTerm の WASM 初期化は非同期で、就緒前の write() はサイレント破棄される。WS の方が先に
  // 繋がると初期プロンプトやバックエンドの一次性失敗通知が消えるため、onReady まで WS を開かない。
  const [ready, setReady] = useState(false);

  useEffect(() => {
    if (!ready) return;
    const scheme = location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${scheme}://${location.host}/api/services/${id}/terminal`);
    ws.binaryType = "arraybuffer";
    wsRef.current = ws;

    // StrictMode の effect 二重実行で新旧 socket が重なるため、遅延 onclose/onmessage が
    // 現役 socket の状態を上書きしないよう毎ハンドラで「自分が現役か」を確認する
    // (cleanup で wsRef を null にしてから閉じる)。
    ws.onopen = () => {
      if (wsRef.current !== ws) return;
      setState("open");
      focus();
    };
    ws.onmessage = (ev) => {
      if (wsRef.current !== ws) return;
      // 出力は Binary(失敗通知も人間可読バイト)。互換のため string も書ける。
      if (ev.data instanceof ArrayBuffer) write(new Uint8Array(ev.data));
      else if (typeof ev.data === "string") write(ev.data);
    };
    ws.onclose = () => {
      if (wsRef.current === ws) setState("closed");
    };

    // unmount で必ず閉じる(バックエンドは input drop → stdin EOF → sh 終了 = ゾンビを残さない)。
    return () => {
      wsRef.current = null;
      ws.close();
    };
  }, [id, ready, write, focus]);

  const send = (data: string | BufferSource) => {
    const ws = wsRef.current;
    if (ws && ws.readyState === WebSocket.OPEN) ws.send(data);
  };

  return (
    <div className="flex flex-col gap-2">
      <span className="text-xs font-semibold text-muted-foreground">
        {state === "open" ? "接続中" : state === "connecting" ? "接続しています…" : "切断"}
      </span>
      <div className="relative">
        {/* @wterm の CSS は index.css で components 層へ降ろしてあるので、utility で
            普通に上書きできる(shadow-none = 既定の黒ドロップシャドウ殺し)。 */}
        <Terminal
          ref={ref}
          autoResize
          cursorBlink
          theme="tsubomi"
          className="h-[480px] rounded-2xl border-2 border-[#e8e2d6] shadow-none"
          onReady={() => setReady(true)}
          onData={(d) => send(enc.encode(d))}
          onResize={(cols, rows) => send(JSON.stringify({ type: "resize", cols, rows }))}
        />
        {/* 切断は端末全面のオーバーレイで知らせる(ヘッダの一行だけでは気づけない)。
            handler は Button だけに置き、`static` + after 疑似要素で当たり判定を
            オーバーレイ全面へ広げる = どこをクリックしても再接続、キーボード/AT の
            意味論もネイティブの button のまま。hover/active は translate-none で殺す —
            translate が none 以外だと Button 自身が after の containing block になり、
            当たり判定がボタン寸法へ塌縮して振動する(translate-y-0 でも 0 は none では
            ないので不可)。autoFocus は Tab 対策:
            フォーカスが terminal に残ると Tab を吞まれてボタンに届かない。 */}
        {state === "closed" && (
          <div className="absolute inset-0 z-10 flex flex-col items-center justify-center gap-3 rounded-2xl bg-background/75 backdrop-blur-[2px]">
            <span className="text-sm font-bold text-foreground">
              切断されました(シェル終了 / タイムアウト)
            </span>
            <Button
              type="primary"
              size="small"
              icon={<RotateCw className="size-4" />}
              onClick={onReconnect}
              autoFocus
              className="static after:absolute after:inset-0 hover:translate-none active:translate-none"
            >
              再接続
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}
