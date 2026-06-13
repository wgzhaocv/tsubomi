// React 19 のドキュメントメタデータ機能を使う。コンポーネント内に書いた
// <title> / <meta> は React が自動で <head> へ巻き上げ・重複排除する。
// ルートごとにこのコンポーネントを置くだけでタブ名・description・OG が切り替わる。
// 静的な favicon / manifest / theme-color は index.html 側(JS 前から効かせるため)。

const SITE = "つぼみ";
const DEFAULT_DESCRIPTION = "社内 PaaS プラットフォーム";
const OG_IMAGE = "/icon-512.png";

export function PageMeta({
  title,
  description = DEFAULT_DESCRIPTION,
}: {
  /** ページ固有名。付けると `<title>` は「{title} · つぼみ」になる。未指定なら「つぼみ」 */
  title?: string;
  description?: string;
}) {
  const full = title ? `${title} · ${SITE}` : SITE;
  return (
    <>
      <title>{full}</title>
      <meta name="description" content={description} />
      <meta property="og:type" content="website" />
      <meta property="og:title" content={full} />
      <meta property="og:description" content={description} />
      <meta property="og:image" content={OG_IMAGE} />
      <meta name="twitter:card" content="summary" />
    </>
  );
}
