import { Activity, Database, HardDrive, Server, Zap, type LucideIcon } from "lucide-react";

import type { TitleColor } from "@/components/ui/title";

// 管理画面のサイドメニューと各リソース画面を「1 つの設定」から駆動するための定義。
// ここを編集すれば、ナビ項目とページ(見出し・空状態)が同時に変わる。
// 並び順は paas-design-v2.md の 4 種リソース(service/database/cache/volume)+
// アクティビティ(操作履歴)。中身は今後 web → CLI の順で実装する(両方から使える)。

export interface ResourceNav {
  /** ルートパス(絶対)。子ルートには先頭の "/" を外して使う。 */
  path: string;
  /** リソース種別(バックエンドの kind に対応)。アクティビティは null。 */
  kind: "service" | "database" | "volume" | "cache" | null;
  /** サイドメニュー / 見出しに出す日本語名 */
  label: string;
  /** サイドメニュー / 空状態アイコン(lucide) */
  icon: LucideIcon;
  /** セクション見出しリボン(Title)の色 */
  ribbon: TitleColor;
  /** 空状態の見出し */
  emptyTitle: string;
  /** 空状態の本文 */
  emptyBody: string;
}

export const RESOURCES: ResourceNav[] = [
  {
    path: "/services",
    kind: "service",
    label: "サービス",
    icon: Server,
    ribbon: "app-teal",
    emptyTitle: "まだサービスがありません",
    emptyBody:
      "GitHub リポジトリと 1 対 1 で結びつくデプロイ単位です。作成すると、ここに一覧で表示されます。",
  },
  {
    path: "/databases",
    kind: "database",
    label: "データベース",
    icon: Database,
    ribbon: "app-blue",
    emptyTitle: "まだデータベースがありません",
    emptyBody:
      "単一インスタンス上に独立した PostgreSQL データベースを作成します。接続文字列はここから確認・コピーできます。",
  },
  {
    path: "/volumes",
    kind: "volume",
    label: "ボリューム",
    icon: HardDrive,
    ribbon: "app-yellow",
    emptyTitle: "まだボリュームがありません",
    emptyBody:
      "サービスのコンテナにマウントして使う永続ディスク領域です。パスを env に注入して利用します。",
  },
  {
    path: "/caches",
    kind: "cache",
    label: "キャッシュ",
    icon: Zap,
    ribbon: "app-orange",
    emptyTitle: "まだキャッシュがありません",
    emptyBody:
      "サービスに注入して使う Valkey の高速キャッシュです。接続文字列を env に注入して利用します。",
  },
  {
    path: "/activity",
    kind: null,
    label: "アクティビティ",
    icon: Activity,
    ribbon: "purple",
    emptyTitle: "まだアクティビティがありません",
    emptyBody: "リソースの作成・削除や owner 操作などの履歴が、ここに時系列で表示されます。",
  },
];
