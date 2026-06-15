import { createBrowserRouter } from "react-router";

import { DashboardLayout } from "@/components/dashboard-layout";
import { RequireOwner } from "@/components/require-owner";
import { RequireViewer } from "@/components/require-viewer";
import { RESOURCES } from "@/lib/resources";
import About from "@/routes/About";
import AdminAudit from "@/routes/AdminAudit";
import AdminOverview from "@/routes/AdminOverview";
import AdminRanking from "@/routes/AdminRanking";
import AdminSettings from "@/routes/AdminSettings";
import CacheDetail from "@/routes/CacheDetail";
import Caches from "@/routes/Caches";
import DatabaseEditor from "@/routes/DatabaseEditor";
import DatabaseLayout from "@/routes/DatabaseLayout";
import DatabaseOverview from "@/routes/DatabaseOverview";
import Databases from "@/routes/Databases";
import DatabaseTables from "@/routes/DatabaseTables";
import Forbidden from "@/routes/Forbidden";
import IpAllowlist from "@/routes/IpAllowlist";
import Login from "@/routes/Login";
import NotFound from "@/routes/NotFound";
import OauthAuthorize from "@/routes/OauthAuthorize";
import OauthCodeCallback from "@/routes/OauthCodeCallback";
import ResourcePage from "@/routes/ResourcePage";
import ServiceDeploys from "@/routes/ServiceDeploys";
import ServiceEnv from "@/routes/ServiceEnv";
import ServiceInjections from "@/routes/ServiceInjections";
import ServiceLayout from "@/routes/ServiceLayout";
import ServiceLogs from "@/routes/ServiceLogs";
import ServiceOverview from "@/routes/ServiceOverview";
import Services from "@/routes/Services";
import Trash from "@/routes/Trash";
import UiGallery from "@/routes/UiGallery";
import VolumeFileBrowser from "@/routes/VolumeFileBrowser";
import VolumeLayout from "@/routes/VolumeLayout";
import VolumeOverview from "@/routes/VolumeOverview";
import Volumes from "@/routes/Volumes";
import Welcome from "@/routes/Welcome";

export const router = createBrowserRouter([
  // ログイン(守衛の外)。
  { path: "/login", element: <Login /> },

  // 社内ドメイン外で弾かれたときの専用画面(守衛の外)。
  // サーバの Google callback がドメイン検証に失敗するとここへリダイレクトする。
  { path: "/forbidden", element: <Forbidden /> },

  // 管理画面の外殻(ログイン守衛 + サイドメニュー)。
  // index = はじめに(CLI 案内)、子 = 各リソース一覧(RESOURCES 設定から生成)。
  {
    path: "/",
    element: <DashboardLayout />,
    children: [
      { index: true, element: <Welcome /> },
      // service 一覧 + 作成導線(M3 S4)。
      { path: "services", element: <Services /> },
      {
        // 詳細の外殻(見出し + サブナビ)。子が 概要 / デプロイ / 注入 / 環境変数 / ログ。
        path: "services/:id",
        element: <ServiceLayout />,
        children: [
          { index: true, element: <ServiceOverview /> },
          { path: "deploys", element: <ServiceDeploys /> },
          { path: "injections", element: <ServiceInjections /> },
          { path: "env", element: <ServiceEnv /> },
          { path: "logs", element: <ServiceLogs /> },
        ],
      },
      // database は実装済み(一覧 + 詳細 3 ページ)。他の種別は当面 ResourcePage の骨格。
      { path: "databases", element: <Databases /> },
      {
        // 詳細の外殻(見出し + サブナビ)。子が 概要 / SQL / テーブル の 3 ページ。
        path: "databases/:id",
        element: <DatabaseLayout />,
        children: [
          { index: true, element: <DatabaseOverview /> },
          { path: "editor", element: <DatabaseEditor /> },
          { path: "tables", element: <DatabaseTables /> },
          { path: "tables/:table", element: <DatabaseTables /> },
        ],
      },
      // volume も実装済み(一覧 + 詳細:概要 / ファイルブラウザ)。
      // ファイルブラウザは splat で假根内のパスを URL にそのまま持つ
      // (/volumes/:id/files/path/to/dir)。
      { path: "volumes", element: <Volumes /> },
      {
        path: "volumes/:id",
        element: <VolumeLayout />,
        children: [
          { index: true, element: <VolumeOverview /> },
          { path: "files", element: <VolumeFileBrowser /> },
          { path: "files/*", element: <VolumeFileBrowser /> },
        ],
      },
      // cache:一覧 + 作成(M5 S1)+ 詳細(接続文字列 / rotate / 削除。M5 S3)。
      // 詳細はタブが概要のみなので単一ページ(Layout/Outlet は使わない)。
      { path: "caches", element: <Caches /> },
      { path: "caches/:id", element: <CacheDetail /> },
      // ゴミ箱は専用ページ(他の未実装リソースは当面 ResourcePage の骨格)。
      { path: "trash", element: <Trash /> },
      // IP 許可リスト(owner 専用のガバナンス画面)。サイドメニューにも owner 限定で出す。
      // バックエンドが 403 で守るので、ここはルート自体は誰でも辿れる(画面側で弾く)。
      { path: "ip-allowlist", element: <IpAllowlist /> },
      // 管制面の可視化(M4 S1 + S5)。匿名化(真名 + 匿名番号 + 使用量、資源名/内容は
      // 出さない)。総覧 + 使用量ランキングは**閲覧**(owner または共有パスワード viewer):
      // <RequireViewer> = 未解錠なら解錠フォームを出す。後端は require_viewer_web で守る。
      {
        element: <RequireViewer />,
        children: [
          { path: "admin", element: <AdminOverview /> },
          { path: "admin/ranking", element: <AdminRanking /> },
        ],
      },
      // 監査ログ(真名 + 操作流水の明文 = §7 匿名化の範囲外)と共有パスワード設定は
      // owner のみ。<RequireOwner> に集約(後端も require_owner_web)。
      {
        element: <RequireOwner />,
        children: [
          { path: "admin/audit", element: <AdminAudit /> },
          { path: "admin/settings", element: <AdminSettings /> },
        ],
      },
      ...RESOURCES.filter(
        (r) =>
          r.kind !== "service" &&
          r.kind !== "database" &&
          r.kind !== "volume" &&
          r.kind !== "cache" &&
          r.path !== "/trash",
      ).map((r) => ({
        path: r.path.replace(/^\//, ""),
        element: <ResourcePage resource={r} />,
      })),
    ],
  },

  // プロジェクト紹介(守衛の外)。同僚にアーキテクチャを見せる公開 1 枚もの。
  // CLI のインストール手順は「はじめに」(/)に集約したので単体 /cli ページは持たない。
  { path: "/about", element: <About /> },

  // CLI ログインフロー(守衛の外)。
  { path: "/oauth/authorize", element: <OauthAuthorize /> },
  { path: "/oauth/code/callback", element: <OauthCodeCallback /> },

  // 開発用スタイル画廊(本番では外す想定)
  { path: "/ui", element: <UiGallery /> },

  // どの route にも該当しないパスは 404 ページへ(catch-all)。
  // 旧 /cli など削除済みパスや打ち間違いを、ブランドを保った 404 で受ける。
  { path: "*", element: <NotFound /> },
]);
