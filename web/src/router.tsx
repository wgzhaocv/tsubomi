import { createBrowserRouter } from "react-router";

import CliInstall from "@/routes/CliInstall";
import Home from "@/routes/Home";
import OauthAuthorize from "@/routes/OauthAuthorize";
import OauthCodeCallback from "@/routes/OauthCodeCallback";
import UiGallery from "@/routes/UiGallery";

export const router = createBrowserRouter([
  { path: "/", element: <Home /> },
  { path: "/cli", element: <CliInstall /> },
  { path: "/oauth/authorize", element: <OauthAuthorize /> },
  { path: "/oauth/code/callback", element: <OauthCodeCallback /> },
  // 開発用スタイル画廊(本番では外す想定)
  { path: "/ui", element: <UiGallery /> },
]);
