import { useEffect, useState } from "react";

// ホスト(サーバ本体)の CPU/メモリ/ディスク使用量を WebSocket で受ける。
// 後端の共有サンプラ(metrics.rs)が 5s 毎にスナップショットを送る。**ページを開いている間
// だけ接続**し、unmount で close する(= 誰も見ていなければ後端のサンプラも止まる)。
// 各値は best-effort:取得不能(dev macOS は /proc 無しで CPU/メモリ)は null → UI は「—」。

// 平台自身の 1 コンテナ(server / pg-platform / valkey …)の使用量。加総せず個別表示。
export type ContainerStat = {
  name: string;
  cpu_pct: number | null;
  mem_bytes: number;
};

export type HostMetrics = {
  cpu_pct: number | null;
  mem_used: number | null;
  mem_total: number | null;
  disk_used: number | null;
  disk_total: number | null;
  disk_pct: number | null;
  // 平台自身(server + infra)の各コンテナ。dev は server が容器でないので出ない。
  platform: ContainerStat[];
};

// 接続状態。WS が開けないと "closed"(rendering 側で控えめに扱う)。
export type HostMetricsState = {
  data: HostMetrics | null;
  connected: boolean;
};

export function useHostMetrics(): HostMetricsState {
  const [state, setState] = useState<HostMetricsState>({ data: null, connected: false });

  useEffect(() => {
    const scheme = location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${scheme}://${location.host}/api/admin/metrics`);

    // open は message より必ず先に発火する(WS 仕様)ので、connected は open で一度立て、
    // message は data だけ差し替える(connected を毎フレーム再設定しない)。
    ws.onopen = () => setState((s) => ({ ...s, connected: true }));
    ws.onmessage = (ev) => {
      try {
        const data = JSON.parse(ev.data as string) as HostMetrics;
        setState((s) => ({ ...s, data }));
      } catch {
        // 壊れたフレームは無視(次のスナップショットで回復する)。
      }
    };
    ws.onclose = () => setState((s) => ({ ...s, connected: false }));

    // unmount(ページ離脱)で必ず閉じる。最後の閲覧者なら後端のサンプラも停止する。
    return () => ws.close();
  }, []);

  return state;
}
