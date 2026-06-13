import { QueryClient } from "@tanstack/react-query";

// アプリ全体で共有する 1 つの QueryClient。サーバ状態(/api/...)はすべてこれが
// キャッシュ・重複排除する。同じ queryKey を複数のコンポーネントが購読しても
// リクエストは 1 回(= props で配って回らなくてよい根拠)。
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // 同一データへの再フェッチを抑える。画面遷移程度では取り直さない。
      staleTime: 60_000,
      // フォーカス復帰での自動再取得は社内ツールでは煩いので止める。
      refetchOnWindowFocus: false,
      retry: 1,
    },
  },
});
