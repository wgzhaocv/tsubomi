-- ゴミ箱は名前を占有しない(2026-08-13、CLI 試用フィードバック起点)。
-- 従来の表級 UNIQUE (user_id, kind, display_name) はゴミ箱内(deleted_at IS NOT NULL)の
-- 行にも効くため、削除 → 同名で作り直しが purge_after(3 日)まで 409 で詰まっていた。
-- 活体だけの部分ユニークインデックスに置き換える:
--   - 活きているリソースの同名は従来どおり 409(名前→id 解決の一意性は不変)。
--   - ゴミ箱内は同名が何行でも堆積できる(subdomain / anon_seq は独立採番なので衝突しない)。
--   - restore 時の活体との衝突はアプリ層(trash.rs)が事前検査 + map_unique で 409 にする。
-- UNIQUE (user_id, kind, anon_seq) は据え置き:anon_seq の MAX+1 採番は deleted_at を
-- 見ない(ゴミ箱行が番号を持ち続ける)前提なので、部分化すると restore で衝突する。

ALTER TABLE resources DROP CONSTRAINT resources_user_id_kind_display_name_key;

CREATE UNIQUE INDEX resources_live_display_name_key
    ON resources (user_id, kind, display_name)
 WHERE deleted_at IS NULL;
