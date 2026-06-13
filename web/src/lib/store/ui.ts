import { create } from "zustand";

// クライアント UI 状態(サーバとは無関係)だけを持つ小さなストア。
// 今は「狭い画面でのサイドメニュー(ドロワー)の開閉」のみ。サーバ状態は
// ここに入れない(それは TanStack Query の担当)。状態は用途ごとに小さく隔離する。
interface UiState {
  /** md 未満でのナビ・ドロワーが開いているか */
  navOpen: boolean;
  openNav: () => void;
  closeNav: () => void;
}

export const useUiStore = create<UiState>((set) => ({
  navOpen: false,
  openNav: () => set({ navOpen: true }),
  closeNav: () => set({ navOpen: false }),
}));
