-- deploys.trigger:その deploy を起こした契機の provenance。
--
-- 解く問題:`DeployTrigger` は既にメモリ上に在るのに表に残していなかったので、reconcile の
-- 復活も、連帯再デプロイ(caller 再リンク)も、部署履歴では**ユーザ自身の再デプロイと
-- 見分けが付かない**。`redeploy()` は再生する版の commit_message をそのまま新しい行へ書くので、
-- 同じ commit 件名の行が並び「なぜ 14:32 に再デプロイされたのか」に答えられない
-- (同 digest の行が複数できて全部が「稼働中」に見えた 2026-07-26 の web 事故と同じ根)。
-- 平台が自動で行を量産する側に回ったこの機能で、旧債が利用者に見える形になった。
--
-- 回填値は**新規行の DEFAULT と同一**('user')。センチネルを使わないのは約束どおり
-- (違う値を使うときは Rust の受け側で既存行から読み戻せることを確認するまで完了ではない
-- — 2026-07-26 の `-infinity` decode panic)。TEXT + CHECK は phase / status と同じ作法。
ALTER TABLE deploys
  ADD COLUMN trigger TEXT NOT NULL DEFAULT 'user'
    CHECK (trigger IN ('user', 'reconcile', 'caller_relink'));
