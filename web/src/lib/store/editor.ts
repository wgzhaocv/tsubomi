import { create } from "zustand";
import { persist } from "zustand/middleware";

import type { QueryResponse } from "@/lib/databases";

// SQL エディタのクライアント状態(サーバ状態ではないので TanStack Query ではなく
// zustand。[[frontend-state-and-components]] の役割分担)。画面遷移で消えないよう、
// SQL 草稿と直近の実行結果を DB ごとに保持する。
//  - height / sqlByDb は localStorage 永続(再読込でも残る)。
//  - resultByDb はメモリのみ。結果は最大 1000 行 × 複数集合で大きくなりうるため
//    localStorage(数 MB 上限)には載せない。遷移では残り、再読込では消える(再実行)。

export const EDITOR_MIN_HEIGHT = 120;
export const EDITOR_MAX_HEIGHT = 800;
const EDITOR_DEFAULT_HEIGHT = 220; // ≒ 10 行

// 直近の実行結果:成功は QueryResponse、失敗はエラー文字列。
export type EditorResult = { ok: QueryResponse } | { error: string };

interface EditorState {
  /** SQL textarea の高さ(px) */
  height: number;
  setHeight: (h: number) => void;
  /** DB(resource id)ごとの SQL 草稿。永続。 */
  sqlByDb: Record<string, string>;
  setSql: (id: string, sql: string) => void;
  /** DB ごとの直近結果。メモリのみ(永続しない)。 */
  resultByDb: Record<string, EditorResult | undefined>;
  setResult: (id: string, result: EditorResult) => void;
}

// 範囲外を弾く(壊れた localStorage 値や極端なドラッグの保険)。
const clampHeight = (h: number) =>
  Math.max(EDITOR_MIN_HEIGHT, Math.min(EDITOR_MAX_HEIGHT, Math.round(h)));

export const useEditorStore = create<EditorState>()(
  persist(
    (set) => ({
      height: EDITOR_DEFAULT_HEIGHT,
      setHeight: (h) => set({ height: clampHeight(h) }),
      sqlByDb: {},
      setSql: (id, sql) => set((s) => ({ sqlByDb: { ...s.sqlByDb, [id]: sql } })),
      resultByDb: {},
      setResult: (id, result) => set((s) => ({ resultByDb: { ...s.resultByDb, [id]: result } })),
    }),
    {
      name: "tbm-sql-editor",
      // 結果(resultByDb)は永続しない — height と SQL 草稿だけ localStorage へ。
      partialize: (s) => ({ height: s.height, sqlByDb: s.sqlByDb }),
    },
  ),
);
