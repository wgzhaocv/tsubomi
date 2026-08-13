import { Terminal, useTerminal, type WTerm } from "@wterm/react";
import { RotateCw } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
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

// 「本当に遅れている」と見なす最下部からの距離(px)。1 行(既定フォントで約 17px)より
// 大きく、2 行より小さい値 — ライブラリが最下部復帰で残す端数(実測 7px)は許し、
// 1 行ぶん流れて見えなくなったら回収する。
const BEHIND_PX = 24;
// 「まだ下を追っている」と見なす距離(px)。BEHIND_PX より緩い(数行ぶんのホイール操作で
// 初めて「自分で過去を読みに行った」と判定する)。
const FOLLOW_PX = 40;

// 1 セッション。WS を張り、端末の入力(onData)を Binary で送り、サーバからの Binary を端末へ書く。
// resize は Text(JSON)で送る。unmount(タブ離脱 / 親の key 更新)で WS を閉じる
// = バックエンドで sh が終了する。再接続は親が key を変えて丸ごと作り直す。
function TerminalPane({ id, onReconnect }: { id: string; onReconnect: () => void }) {
  // write/focus は useCallback([]) の恒等安定(呼び出し時に ref 経由で実体へ届く)なので、
  // effect の依存に入れても貼り直しは起きない。
  const { ref, write, focus } = useTerminal();
  const wsRef = useRef<WebSocket | null>(null);
  // 端末のスクロール要素(WTerm.element)。**最下部への貼り付けを自前でやる**ため保持する:
  // ライブラリは「書く直前に最下部にいたか」で追従を決めるが、その最下部復帰は scrollTop を
  // 行高の倍数へ丸める(最大 1 行ぶん手前に着地する)。判定の許容は 5px しかないので、
  // 一度その位置に着くと以後ずっと「下にいない」扱いになり、**新しい出力が見えなくなる**
  // (ユーザ報告 2026-08-13)。こちらで書き込みのたびに貼り直す。
  const scrollElRef = useRef<HTMLElement | null>(null);
  const stickPendingRef = useRef(false);
  // 「最下部に貼り付いて追う」状態か。**scroll イベントで更新する**のが要点:
  // 書き込み時点で測ると resize 経路(下記 onResize)では測れない — ライブラリの resize は
  // 先に描画コンテナを空にするので、その瞬間の距離は常に 0 に見えるため。
  const followRef = useRef(true);
  const wtRef = useRef<WTerm | null>(null);
  // 接続前に発生した送信(初回 resize / 早すぎる打鍵)の待ち行列。onopen で流す。
  const queueRef = useRef<(string | BufferSource)[]>([]);
  const [state, setState] = useState<ConnState>("connecting");
  // WTerm の WASM 初期化は非同期で、就緒前の write() はサイレント破棄される。WS の方が先に
  // 繋がると初期プロンプトやバックエンドの一次性失敗通知が消えるため、onReady まで WS を開かない。
  const [ready, setReady] = useState(false);

  // 最下部へ貼り付ける。ライブラリの描画は write → setTimeout(0) → rAF なので、同じ順で
  // 1 つ後ろに並べて**描画後**の scrollHeight で見る。
  //
  // ★ **ライブラリの着地点と張り合わない**のが要点(ユーザ報告「打鍵のたびに揺れる」):
  // ライブラリは入力のたびに scrollTop を行高の倍数へ丸めて最下部へ戻す(= 実測で真の
  // 最下部より 7px 手前)。そこへ毎回 scrollTop = scrollHeight を代入すると、1 打鍵ごとに
  // 7px 上下する。行 1 つぶんの端数は「もう最下部」と見なし、**本当に遅れた時だけ**直す。
  // 遅れは 1 行(≈17px)ずつ増えるので、追従が止まっていれば 1 行遅れで必ず回収できる。
  // 一度こちらが真の最下部へ戻すと、ライブラリ自身の追従判定(許容 5px)も復活する。
  const stickToBottom = useCallback(() => {
    if (stickPendingRef.current) return;
    stickPendingRef.current = true;
    setTimeout(() => {
      requestAnimationFrame(() => {
        stickPendingRef.current = false;
        const el = scrollElRef.current;
        if (!el) return;
        if (el.scrollHeight - el.scrollTop - el.clientHeight > BEHIND_PX) {
          el.scrollTop = el.scrollHeight;
        }
      });
    }, 0);
  }, []);

  // 追従状態はユーザのスクロール(と貼り付けの結果)から拾う。許容を広めに取るのは、
  // ライブラリが最下部復帰で残す端数(実測 7px)を「まだ下にいる」と見なすため —
  // ここが 5px しかないのがライブラリ側の追従が止まる原因だった。
  useEffect(() => {
    const el = scrollElRef.current;
    if (!ready || !el) return;
    const onScroll = () => {
      followRef.current = el.scrollHeight - el.scrollTop - el.clientHeight <= FOLLOW_PX;
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, [ready]);

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
      // **接続前に溜めた制御フレームをここで流す**。端末の ResizeObserver は init の次フレーム
      // (≤16ms)で最初の resize を出すが、WS はまだ CONNECTING(本番は CF Tunnel 経由で
      // 数十 ms)。捨てると PTY は docker 既定の 80×24 のままになり、表示は 26 行 100 列なのに
      // shell は 24 行 80 列だと思い込む(折り返し・vi/top の描画崩れ)。dev は握手が速くて
      // 表に出ない = 本番だけで起きる型(review 2026-08-13)。
      for (const q of queueRef.current) ws.send(q);
      queueRef.current = [];
      focus();
    };
    ws.onmessage = (ev) => {
      if (wsRef.current !== ws) return;
      const follow = followRef.current;
      // 出力は Binary(失敗通知も人間可読バイト)。互換のため string も書ける。
      if (ev.data instanceof ArrayBuffer) write(new Uint8Array(ev.data));
      else if (typeof ev.data === "string") write(ev.data);
      if (follow) stickToBottom();
    };
    ws.onclose = () => {
      if (wsRef.current === ws) setState("closed");
    };

    // unmount で必ず閉じる(バックエンドは input drop → stdin EOF → sh 終了 = ゾンビを残さない)。
    return () => {
      wsRef.current = null;
      ws.close();
    };
  }, [id, ready, write, focus, stickToBottom]);

  const send = (data: string | BufferSource) => {
    const ws = wsRef.current;
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(data);
      return;
    }
    // 未接続なら少しだけ溜めて onopen で流す(初回 resize と、開く前に叩かれたキー)。
    // 上限は暴走防止のためだけ — 溢れたら古い方から捨てる。
    if (queueRef.current.length >= 32) queueRef.current.shift();
    queueRef.current.push(data);
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
          onReady={(wt: WTerm) => {
            // スクロール要素は WTerm が持つ本体 div(= この Terminal が描く要素)。
            wtRef.current = wt;
            scrollElRef.current = wt.element;
            setReady(true);
          }}
          // 初期化に失敗したら「接続しています…」で固まらせない(WASM 不可・CSP 等)。
          // 切断オーバーレイに倒して再接続の入口を出す。
          onError={() => setState("closed")}
          onData={(d) => send(enc.encode(d))}
          onResize={(cols, rows) => {
            send(JSON.stringify({ type: "resize", cols, rows }));
            // ライブラリの resize は描画コンテナを空にするので、ブラウザが scrollTop を 0 へ
            // 詰める。追従中だったら貼り直す(貼らないと履歴の先頭で固まり、以後の出力も
            // 追わなくなる)。ここで距離を測り直してはいけない — 空の瞬間は常に「最下部」に見える。
            if (followRef.current) stickToBottom();
          }}
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
