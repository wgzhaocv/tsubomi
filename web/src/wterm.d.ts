// @wterm/react のスタイルは subpath export(`.css` ファイルに解決)。指定子が `.css` で
// 終わらないため vite/client の `*.css` 宣言に当たらず、TS が副作用 import の型を見つけられない。
// その最小の補い(値は import しない = 型なしで十分)。
declare module "@wterm/react/css";
