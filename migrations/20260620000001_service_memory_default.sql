-- service の既定メモリ硬上限を 512MB → 1024MB(1GB)へ引き上げる。
--
-- 理由:香橙派(Pi)上の実 app を実測すると bun/Next.js は 60〜500MB を使い、特に重い
-- bun サービスは起動直後から ~500MB に達する。512MB 既定では一峰で OOM しやすかった。
-- 1GB = 最重実測(~500MB)の約 2 倍の余裕。
--
-- 硬上限の「仕組み」自体は維持する(単機・ホスト直走りの管制面を守る一線。CLAUDE.md
-- の「破ってはいけない一線」)。上限を外すのではなく既定を上げるだけ。
--
-- memory_mb はこれまで API / CLI で個別設定できず常にこの列既定を使っていたため、既存行の
-- 512 は全て旧既定 = 一律 1024 へ引き上げて安全(意図的に 512 を選んだ行は存在しない)。
-- メモリは**コンテナ起動の瞬間に適用**されるので、既存 service は次回デプロイで 1GB になる。
ALTER TABLE service_details ALTER COLUMN memory_mb SET DEFAULT 1024;
UPDATE service_details SET memory_mb = 1024 WHERE memory_mb = 512;
