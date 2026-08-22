// subdomain の形式検証(クライアント側の**前置門**)。作成フォームと変更 modal が共有する。
//
// **権威はサーバ**(`crates/server/src/services/mod.rs::validate_subdomain` /
// `reserved_subdomain`)。ここは「明らかに不正な値でサーバへ往復させない」ための UX 層で、
// 判定の単一真源ではない — サーバは今後も同じ値を 400 で弾く。だから漂移したときの害は
// **緩すぎる方向だけ**(サーバが受け止める)で、厳しすぎる方向は「正しい値が入力できない」
// = 実害になる。サーバ側の規則・予約語を変えたら**ここも変える**。
//
// port / memory をクライアントで先に検証しているのと同じ理由ではない(あちらは Number("abc")
// が NaN → null → サーバが「省略」と同一視して黙って既定に倒れるため必須の門)。こちらは
// サーバが正しく 400 を返すので、目的は純粋に「押しても何も起きない」を「なぜ押せないか」に
// 変えることにある。

/** `MAX_SUBDOMAIN_LEN`(サーバ)と同値。slugify の切り詰めと同じ 50。 */
const MAX_SUBDOMAIN_LEN = 50;
/** `RESERVED_SUBDOMAINS`(サーバ)と同値。加えて `tsubomi-` 前綴も予約。 */
const RESERVED_SUBDOMAINS = ["paas", "registry", "traefik", "www", "api", "db", "cache"];

/**
 * 形式上の問題があれば理由(日本語)、無ければ null。
 *
 * 空文字は「まだ入力していない」= 問題として出さない(赤字を出すのは入力を始めてから)。
 * 呼び出し側は空を submit しないこと。**同値**はここでは扱わない — 形式の問題ではなく
 * 「変更するものが無い」という別の状態なので、呼び出し側が現在値と比べる。
 */
export function subdomainProblem(value: string): string | null {
  if (value === "") return null;
  if (!/^[a-z][a-z0-9-]*$/.test(value)) {
    return "小文字英数と「-」だけ・英字始まりにしてください";
  }
  if (value.endsWith("-")) return "「-」で終わることはできません";
  // 非 ASCII は上の正規表現で既に落ちているので、ここでは length = 文字数(サーバの
  // `chars().count()` と一致する)。
  if (value.length > MAX_SUBDOMAIN_LEN) {
    return `${MAX_SUBDOMAIN_LEN} 文字以内にしてください(現在 ${value.length} 文字)`;
  }
  if (RESERVED_SUBDOMAINS.includes(value) || value.startsWith("tsubomi-")) {
    return "この名前は予約されています(プラットフォーム / インフラ名と衝突)";
  }
  return null;
}
