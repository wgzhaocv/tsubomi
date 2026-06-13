# UI デザイン・リファレンス(animal-island-ui 移植)

`web/src/components/ui/*` のコンポーネントは **animal-island-ui**(guokaigdg、
どうぶつの森風)の見た目・実装を tsubomi の **Tailwind v4 + shadcn 構造**へ移植した
もの。新しいコンポーネントを足すときも、ここの規約に沿えば同じ風格で作れる。

## このフォルダの中身(原典から複製した設計ドキュメント)

| ファイル           | 用途                                                                                                                                   |
| ------------------ | -------------------------------------------------------------------------------------------------------------------------------------- |
| `SKILL.md`         | **最重要**。全デザイントークン(色・角丸・影・font・spacing)と、各コンポーネントの CSS 仕様。新コンポーネントの寸法・配色はここを見る。 |
| `DESIGN_PROMPT.md` | 配色 / フォント / サイズ表。AI 作図ツール(v0・Figma AI・Midjourney 等)へ渡す視覚スタイル記述。ロゴ/画像生成の prompt の素にも使える。  |
| `AI_USAGE.md`      | 原典コンポーネントの props / import / 既定値(verbatim)。API を合わせるときの照合用。                                                   |

原典のフルソース(`.tsx` + `.module.less` 全部)はローカル clone にある:
`~/Desktop/projects/animal-island-ui/src/components/<名前>/`。

## tsubomi 側の地基(どこに何があるか)

- **デザイントークン**:`web/src/index.css` の `:root`。原典 `variables.less` の値を
  反映済み(主色 `#19c8b9`、文字 `#794f27` / 本文 `#725d42`、カード面
  `rgb(247,243,223)`、罫線 `#aaa69d`/`#c4b89e`、focus 黄 `#f5c31c`、error
  `#e05a5a/#e87878/#c94444` など)。Tailwind の意味トークン(`bg-card` 等)へ
  `@theme inline` でマップ。
- **bespoke CSS**(Tailwind 任意値で書けないもの)も `index.css` に集約:Title リボン
  (`.tbm-ribbon*`)、Tooltip 矢印/しっぽ(`.tbm-tooltip-*`)、各 `@keyframes`
  (`animal-cbx-splash` / `animal-radio-splash` / `animal-leaf-wiggle` /
  `animal-zoom-in` / `animal-fade-in` / `animal-btn-loading` /
  `animal-time-*` / `tbm-select-cursor-slide-in`)、`prefers-reduced-motion` の
  一括無効化。
- **背景壁紙**:`web/public/bg-content.jpg`(原典 docs の淡緑+三角)。
- **アイコン**:`icons.tsx`(NookPhone 9 種、tree-shakeable な個別 export)。
  葉っぱは `public/icons/icon-leaf.png`、カーソルは `public/cursor/select-cursor.svg`。
- **スタイル画廊**:ルート `/ui`(`routes/UiGallery.tsx`)で全 variant を確認できる。
- **a11y 指針**:同フォルダの `../a11y-audit.md`(Codex 監査)。

## 新しい AC 風コンポーネントの作り方

1. 原典に同等品があれば `~/Desktop/projects/animal-island-ui/src/components/<名前>/`
   の `.tsx` + `.module.less` を読む。無ければ `SKILL.md` のトークンで組み立てる。
2. `web/src/components/ui/<name>.tsx` に作る。**原典の prop 式 API を踏襲**
   (例:Button は `type/size/danger/ghost/loading/block/icon`)。
3. スタイルは **Tailwind v4 の任意値で原典の hex/px を厳密に再現**
   (例:`bg-[rgb(247,243,223)]`、`shadow-[0_5px_0_0_#bdaea0]`)。色は発明しない。
   `var(--animal-*)` / less `@var` は `variables.less` のリテラルへ解決する。
4. クラス結合は `@/lib/utils` の `cn`。`import * as React from "react"`。
   コメントは日本語。新依存は足さない(radix は既存の react-slot のみ)。
5. **focus は outline で**(`focus-visible:[outline:2px_solid_#19c8b9]` など)。
   `ring`(box-shadow)は 3D 影と衝突するので使わない。
6. アニメーションの keyframe / bespoke CSS が要るなら `index.css` に足す。
   `prefers-reduced-motion` は既に一括で無効化される。
7. a11y:アクセシブル名(label/aria-label)、キーボード操作、role/ARIA 状態を
   `a11y-audit.md` の指針に沿って付ける。色のコントラストだけは別途調整待ち。
8. クラスの canonical 化(`z-[100]`→`z-100` 等)は Tailwind LSP の指摘に従う。
9. 仕上げに `bunx vp fmt` → `bunx vp lint` → `bun run build`、`/ui` 画廊に足して目視。

> 注意:これらは個人学習ライブラリ(任天堂 AC 由来の意匠)の移植。社内利用は問題
> ないが、対外公開時は意匠/フォント等のライセンスに留意する。
