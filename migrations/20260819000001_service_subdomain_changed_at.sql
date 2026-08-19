-- service subdomain の作成後変更に伴う「注入の未反映」判定材料。
-- caller が注入している service(callee)の subdomain が、caller の serving コンテナ起動より
-- **後**に変わった = 注入済みの `_URL` / `_HOST` は旧値 → 未反映(要デプロイ)として出す
-- (cache の rotated_at と同型。list_injections の GREATEST に参加する)。
--
-- 回填値 = 新規行 DEFAULT と同値(epoch)。既存 service は「変更されたことがない」= 未反映を
-- 誤報しない。epoch は DateTime<Utc> で読み戻せる有限値(-infinity 事故 2026-07-26 の約束事)。
ALTER TABLE service_details
    ADD COLUMN subdomain_changed_at TIMESTAMPTZ NOT NULL DEFAULT 'epoch',
    ADD CONSTRAINT service_details_subdomain_changed_at_finite
    CHECK (subdomain_changed_at > '-infinity'::timestamptz
       AND subdomain_changed_at < 'infinity'::timestamptz);
