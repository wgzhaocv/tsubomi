import { create } from "zustand";
import { persist } from "zustand/middleware";

// SQL エディタの UI 設定(クライアント状態)。高さはドラッグバーで変えられ、
// localStorage に同期して再訪時も保つ。サーバ状態ではないので TanStack Query
// ではなく zustand に置く([[frontend-state-and-components]] の役割分担)。

export const EDITOR_MIN_HEIGHT = 120;
export const EDITOR_MAX_HEIGHT = 800;
const EDITOR_DEFAULT_HEIGHT = 220; // ≒ 10 行

interface EditorState {
  /** SQL textarea の高さ(px) */
  height: number;
  setHeight: (h: number) => void;
}

// 範囲外を弾く(壊れた localStorage 値や極端なドラッグの保険)。
const clampHeight = (h: number) =>
  Math.max(EDITOR_MIN_HEIGHT, Math.min(EDITOR_MAX_HEIGHT, Math.round(h)));

export const useEditorStore = create<EditorState>()(
  persist(
    (set) => ({
      height: EDITOR_DEFAULT_HEIGHT,
      setHeight: (h) => set({ height: clampHeight(h) }),
    }),
    { name: "tbm-sql-editor" },
  ),
);
