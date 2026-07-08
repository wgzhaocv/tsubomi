-- service の直近デプロイ源(provenance = 最後に使ったレシピ)を管制面に残す。
--   'push'       = 従来経路(CI / tbm deploy --local が registry へ push し hook が digest を運ぶ)
--   'image'      = 既成イメージ参照(source_spec = 例 pgvector/pgvector:pg17)。サーバ側で pull。
--   'dockerfile' = コンテキスト無し Dockerfile(source_spec = Dockerfile 全文)。サーバ側で build。
-- レシピは数百バイトの文文なのでファイルではなく DB に置く(ファイル真源を増やさない)。
-- ※注意:これは「最後に deploy-source で使った値」であって完全な期望状態ではない —
--   hook/--local 経路は書き戻さないので、経路 1/2 に戻ると値は前回のまま残る(provenance 用途)。

ALTER TABLE service_details
    ADD COLUMN source_kind TEXT NOT NULL DEFAULT 'push'
        CHECK (source_kind IN ('push', 'image', 'dockerfile')),
    ADD COLUMN source_spec TEXT;
