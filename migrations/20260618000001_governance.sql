-- tsubomi M4:ガバナンス(可視化 + 最後の砦 + audit 閲覧)のスキーマ。
-- 背骨(doc/paas-m4-design.md §2):可視性(見える)と兜底(動かす)の 2 枚。
-- M1 で audit_log は既に在る(actor/action/target_resource/target_user/detail)。本 migration は
--   * platform_config    … 平台設定(磁盘告警の去重状態 / 共有 viewer パスワード hash など)
--   * admin_action_codes … owner の危険操作(他人の資源 stop/delete)の二段確認コード
--   * audit_log の閲覧フィルタ用 index(actor_id / target_user)
-- を足す。owner ガバナンスは web 専用、後端が毎回 owner を検証(前端表示はただの UX)。

-- ===========================================================================
-- platform_config(key → jsonb)。tech-design §2 の定義そのまま(M0 DDL に載っていたが
--   migration には未投入だったのでここで作る)。M4 で使うキー:
--     'disk_alert_state' = { "level": "ok"|"warn"|"critical", "notified_at": <ts>, "used_pct": <n> }
--     'viewer_password'  = { "hash": "<bcrypt>", "updated_at": <ts>, "updated_by": "<uuid>" }  (S5・否決可)
-- ===========================================================================

CREATE TABLE platform_config (
    key        TEXT        PRIMARY KEY,
    value      JSONB       NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ===========================================================================
-- admin_action_codes(危険操作の検証コード。単回消費 = authcodes と同じ Postgres 流儀)。
--   owner が他人の資源を stop/delete する二段確認の 1 段目で 1 行 INSERT、2 段目で
--   DELETE..RETURNING(単回消費 + 期限 + 文脈一致を 1 文で)。期限切れは gc が掃除。
--   code は平文を保存せず sha256(他の token と同じ規律)。resource_id は soft 削除済みでも
--   参照したいので FK を張らない(actor_id だけ FK)。
-- ===========================================================================

CREATE TABLE admin_action_codes (
    code_hash   TEXT        PRIMARY KEY,                 -- sha256(6 桁コード) の hex
    actor_id    UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    resource_id UUID        NOT NULL,                    -- 対象資源(resources.id)
    action      TEXT        NOT NULL CHECK (action IN ('stop','delete')),
    expires_at  TIMESTAMPTZ NOT NULL,                    -- now() + 10min
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX ON admin_action_codes (expires_at);

-- ===========================================================================
-- audit_log の閲覧用 index(表自体は M1 既存:created_at DESC / target_resource)。
--   owner の監査ログ画面が actor / target_user でフィルタできるように 2 本足す。
-- ===========================================================================

CREATE INDEX ON audit_log (actor_id);
CREATE INDEX ON audit_log (target_user);
