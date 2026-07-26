-- 2026-07-26 の事故の**恒久封鎖**:Postgres の `infinity` / `-infinity` は `DateTime<Utc>` に
-- 読み込めず sqlx が panic する。しかもこの平台は `panic = "abort"`(ワークスペースの release
-- profile)なので、**1 リクエストの panic で server プロセスが落ちる**(docker の
-- restart=unless-stopped が拾うが、進行中の deploy は失敗扱いになり WS も切れる)。
--
-- 読み側の丸め(`LEAST(GREATEST(…))`)は SELECT ごとに書き忘れられるので、**書き込みを拒む**方を
-- 本体にする。CHECK なら手 SQL(runbook は `docker exec psql` 前提)・将来の migration の回填・
-- dump の復元、どの経路でも止まり、panic ではなく普通の SQL エラー(= 観測できる 500)になる。
--
-- 対象は「Rust が `DateTime<Utc>` で受ける列」のうち、**アプリ以外の経路で値が入り得るもの**。
-- 既に epoch へ寄せてあるので既存行はすべて通る(20260726000002)。
ALTER TABLE injections
    ADD CONSTRAINT injections_created_at_finite
    CHECK (created_at > '-infinity'::timestamptz AND created_at < 'infinity'::timestamptz);

ALTER TABLE deploys
    ADD CONSTRAINT deploys_created_at_finite
    CHECK (created_at > '-infinity'::timestamptz AND created_at < 'infinity'::timestamptz),
    ADD CONSTRAINT deploys_finished_at_finite
    CHECK (finished_at IS NULL
           OR (finished_at > '-infinity'::timestamptz AND finished_at < 'infinity'::timestamptz));
