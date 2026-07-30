// 表示用フォーマッタの置き場(リソース横断で使う純関数だけ。fetch や Query は置かない)。

// バイト数を人間可読に(一覧カード・ファイルブラウザ・管理概要で共用)。
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(1)} ${units[i]}`;
}

// ISO 時刻 → 絶対日付(ja-JP)。一覧カードのフッタ等、日付だけ見せたい場面の単一の書式。
export function formatDate(iso: string): string {
  return new Date(iso).toLocaleDateString("ja-JP");
}

// ISO 時刻 → 相対表記(「3日前」「5時間前」「たった今」)。一覧カードの
// 最終デプロイ / rotate 表示用。粗い粒度でよい(分未満は「たった今」、30 日を
// 超えたら絶対日付に切り替える — 「200日前」は日付の方が読める)。
export function formatRelative(iso: string): string {
  const then = new Date(iso);
  if (Number.isNaN(then.getTime())) return iso;
  const minutes = Math.floor((Date.now() - then.getTime()) / 60_000);
  if (minutes < 1) return "たった今";
  if (minutes < 60) return `${minutes}分前`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}時間前`;
  const days = Math.floor(hours / 24);
  if (days <= 30) return `${days}日前`;
  return formatDate(iso);
}
