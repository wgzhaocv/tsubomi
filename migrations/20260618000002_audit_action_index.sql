-- tsubomi M4 S4:監査ログ閲覧の action 前方一致フィルタ(`action LIKE 'owner.%'` 等)を
-- index 可能にする。text_pattern_ops は前方一致 LIKE を B-tree で引けるようにする演算子クラス
-- (照合ロケールに依らず 'prefix%' を index range scan できる)。audit_log は増える一方なので、
-- フィルタが線形劣化しないようここで張る。既存 index(created_at DESC / target_resource /
-- actor_id / target_user)はそのまま。

CREATE INDEX ON audit_log (action text_pattern_ops);
