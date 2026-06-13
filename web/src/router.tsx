import { createBrowserRouter } from "react-router";

import { DashboardLayout } from "@/components/dashboard-layout";
import { RESOURCES } from "@/lib/resources";
import CliInstall from "@/routes/CliInstall";
import Forbidden from "@/routes/Forbidden";
import Login from "@/routes/Login";
import OauthAuthorize from "@/routes/OauthAuthorize";
import OauthCodeCallback from "@/routes/OauthCodeCallback";
import ResourcePage from "@/routes/ResourcePage";
import UiGallery from "@/routes/UiGallery";
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
      ...RESOURCES.map((r) => ({
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
