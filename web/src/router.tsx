import { createBrowserRouter } from "react-router";

import { DashboardLayout } from "@/components/dashboard-layout";
import { RESOURCES } from "@/lib/resources";
import CliInstall from "@/routes/CliInstall";
import DatabaseEditor from "@/routes/DatabaseEditor";
import DatabaseLayout from "@/routes/DatabaseLayout";
import DatabaseOverview from "@/routes/DatabaseOverview";
import Databases from "@/routes/Databases";
import DatabaseTables from "@/routes/DatabaseTables";
import Forbidden from "@/routes/Forbidden";
import IpAllowlist from "@/routes/IpAllowlist";
import Login from "@/routes/Login";
import OauthAuthorize from "@/routes/OauthAuthorize";
import OauthCodeCallback from "@/routes/OauthCodeCallback";
import ResourcePage from "@/routes/ResourcePage";
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
      // service 一覧 + 作成導線(M3 S4)。詳細・ログ・停止/再開は後フェーズ。
      { path: "services", element: <Services /> },
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
      // ゴミ箱は専用ページ(他の未実装リソースは当面 ResourcePage の骨格)。
      { path: "trash", element: <Trash /> },
      // IP 許可リスト(owner 専用のガバナンス画面)。サイドメニューにも owner 限定で出す。
      // バックエンドが 403 で守るので、ここはルート自体は誰でも辿れる(画面側で弾く)。
      { path: "ip-allowlist", element: <IpAllowlist /> },
      ...RESOURCES.filter(
        (r) =>
          r.kind !== "service" &&
          r.kind !== "database" &&
          r.kind !== "volume" &&
          r.path !== "/trash",
      ).map((r) => ({
        path: r.path.replace(/^\//, ""),
        element: <ResourcePage resource={r} />,
      })),
    ],
  },

  // CLI フロー / 単体ページ(守衛の外)。
  { path: "/cli", element: <CliInstall /> },
  { path: "/oauth/authorize", element: <OauthAuthorize /> },
  { path: "/oauth/code/callback", element: <OauthCodeCallback /> },

  // 開発用スタイル画廊(本番では外す想定)
  { path: "/ui", element: <UiGallery /> },
]);
