import { useEffect, useState } from "react";

// ホスト(サーバ本体)の CPU/メモリ/ディスク使用量を WebSocket で受ける。
// バックエンドの共有サンプラ(metrics.rs)が 5s 毎にスナップショットを送る。**ページを開いている間
// だけ接続**し、unmount で close する(= 誰も見ていなければバックエンドのサンプラも止まる)。
// 各値は best-effort:取得不能(dev macOS は /proc 無しで CPU/メモリ)は null → UI は「—」。

// プラットフォーム自身の 1 コンテナ(server / pg-platform / valkey …)の使用量。合算せず個別表示。
export type ContainerStat = {
  name: string;
  /** **ホスト全体に対する** CPU 使用率(%)。サーバが正規化済み(docker の生値は 100% = 1 コア)。 */
  cpu_pct_host: number | null;
  mem_bytes: number;
};

// 温度センサ 1 つ。label は内核が付けた名前そのまま(thermal zone の type /
// hwmon のチップ名 — 機種別対応表は持たない)。
export type TempSensor = {
  label: string;
  temp_c: number;
};

export type HostMetrics = {
  /** ホスト全体に対する CPU 使用率(%)。/proc/stat = 全コア合算。 */
  cpu_pct_host: number | null;
  /** ホストの論理コア数(/proc/stat の cpu 行数 = 上の % と同じ出典)。 */
  host_cores: number | null;
  mem_used: number | null;
  mem_total: number | null;
  disk_used: number | null;
  disk_total: number | null;
  disk_pct: number | null;
  // プラットフォーム自身(server + infra)の各コンテナ。dev は server がコンテナでないので出ない。
  platform: ContainerStat[];
  // ホスト温度。取得不能(dev macOS / VM)は空 = 行ごと非表示。
  temps: TempSensor[];
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

    // unmount(ページ離脱)で必ず閉じる。最後の閲覧者ならバックエンドのサンプラも停止する。
    return () => ws.close();
  }, []);

  return state;
}
