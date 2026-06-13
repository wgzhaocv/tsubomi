import { Activity, Database, HardDrive, Server, Zap, type LucideIcon } from "lucide-react";

import type { TitleColor } from "@/components/ui/title";

// 管理画面のサイドメニューと各リソース画面を「1 つの設定」から駆動するための定義。
// ここを編集すれば、ナビ項目とページ(見出し・空状態・作成コマンド)が同時に変わる。
// 並び順は paas-design-v2.md の 4 種リソース(service/database/cache/volume)+
// アクティビティ(操作履歴)。内容は M1〜M5 で順次実装するため、今は空状態のみ。

export interface ResourceNav {
  /** ルートパス(絶対)。子ルートには先頭の "/" を外して使う。 */
  path: string;
  /** CLI の <kind>(作成コマンドの語幹)。アクティビティは null。 */
  kind: "service" | "database" | "volume" | "cache" | null;
  /** サイドメニュー / 見出しに出す日本語名 */
  label: string;
  /** 1 行の補助説明 */
  tagline: string;
  /** サイドメニュー / 空状態アイコン(lucide) */
  icon: LucideIcon;
  /** セクション見出しリボン(Title)の色 */
  ribbon: TitleColor;
  /** 空状態の見出し */
  emptyTitle: string;
  /** 空状態の本文 */
  emptyBody: string;
  /** 空状態で見せる作成コマンド(CLI 中心の設計のため)。履歴系は無し。 */
  createHint?: string;
}

export const RESOURCES: ResourceNav[] = [
  {
    path: "/services",
    kind: "service",
    label: "サービス",
    tagline: "デプロイした app(コンテナ)",
    icon: Server,
    ribbon: "app-teal",
    emptyTitle: "まだサービスがありません",
    emptyBody:
      "GitHub リポジトリと 1 対 1 で結びつくデプロイ単位です。CLI から作成すると、ここに一覧で表示されます。",
    createHint: "tbm service create <名前>",
  },
  {
    path: "/databases",
    kind: "database",
    label: "データベース",
    tagline: "PostgreSQL のデータベース",
    icon: Database,
    ribbon: "app-blue",
    emptyTitle: "まだデータベースがありません",
    emptyBody:
      "単一インスタンス上に独立した PostgreSQL データベースを作成します。接続文字列はここから確認・コピーできます。",
    createHint: "tbm database create <名前>",
  },
  {
    path: "/volumes",
    kind: "volume",
    label: "ボリューム",
    tagline: "ファイル保存用の永続ディスク",
    icon: HardDrive,
    ribbon: "app-yellow",
    emptyTitle: "まだボリュームがありません",
    emptyBody:
      "サービスのコンテナにマウントして使う永続ディスク領域です。パスを env に注入して利用します。",
    createHint: "tbm volume create <名前>",
  },
  {
    path: "/caches",
    kind: "cache",
    label: "キャッシュ",
    tagline: "Valkey の高速キャッシュ",
    icon: Zap,
    ribbon: "app-orange",
    emptyTitle: "まだキャッシュがありません",
    emptyBody:
      "サービスに注入して使う Valkey の高速キャッシュです。接続文字列を env に注入して利用します。",
    createHint: "tbm cache create <名前>",
  },
  {
    path: "/activity",
    kind: null,
    label: "アクティビティ",
    tagline: "操作の履歴とリソースの状況",
    icon: Activity,
    ribbon: "purple",
    emptyTitle: "まだアクティビティがありません",
    emptyBody: "リソースの作成・削除や owner 操作などの履歴が、ここに時系列で表示されます。",
  },
];
