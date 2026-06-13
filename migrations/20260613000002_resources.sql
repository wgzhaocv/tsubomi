-- tsubomi M1:リソースのスーパーテーブル + database 詳細 + 監査ログ。
-- 背骨(tech-design §2):4 種のリソースは resources スーパーテーブル 1 枚 +
-- 種別毎の detail テーブル。M1 では database の detail だけを足す(service /
-- cache / volume と env / injections / deploys は各フェーズで追加 — §9 の規律)。

-- ===========================================================================
-- resources(スーパーテーブル)
--   注入の FK・管理画面の一括クエリ・ゴミ箱のロジックを 1 組に統一する。
-- ===========================================================================

CREATE TABLE resources (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      UUID        NOT NULL REFERENCES users(id),
    kind         TEXT        NOT NULL CHECK (kind IN ('service','database','cache','volume')),
    display_name TEXT        NOT NULL,                  -- ユーザの自由名(改名は接続文字列に触れない)
    anon_seq     INT         NOT NULL,                  -- 匿名番号(user+kind 内連番):database1/2…
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at   TIMESTAMPTZ,                           -- 非 NULL = ゴミ箱の中
    purge_after  TIMESTAMPTZ,                           -- = deleted_at + 3d。reconcile が期限到来で物理削除
    trash_meta   JSONB,                                 -- 復元に必要なもの(dump パス / 名前など)
    UNIQUE (user_id, kind, display_name),
    UNIQUE (user_id, kind, anon_seq),
    UNIQUE (id, kind)                                   -- detail / injections の「kind 付き複合 FK」用
);

CREATE INDEX ON resources (user_id, kind);
-- ゴミ箱の期限到来掃除(reconcile)が引く行だけを対象にした部分インデックス。
CREATE INDEX ON resources (purge_after) WHERE purge_after IS NOT NULL;

-- ===========================================================================
-- database_details(resources と 1:1)
--   pg_dbname は db_<shortid>(pg-tenant 内の実 DB 名)。display_name とは別 —
--   単一インスタンスでグローバル一意な wire 名が要るため。
-- ===========================================================================

CREATE TABLE database_details (
    resource_id UUID        PRIMARY KEY,
    kind        TEXT        NOT NULL DEFAULT 'database' CHECK (kind = 'database'),
    pg_dbname   TEXT        NOT NULL UNIQUE,
    rotated_at  TIMESTAMPTZ,                            -- human role の最後の rotate 時刻(UI の失効ソフト提示)
    FOREIGN KEY (resource_id, kind) REFERENCES resources(id, kind) ON DELETE CASCADE
);

-- ===========================================================================
-- database_roles(1 つの DB に 2 つの登録資格情報。tech-design §2 の改訂)
--   app   = 内部:デプロイ済み service に注入(M3)、内部路径、既定 rotate なし。
--   human = 外部:ローカル開発 / DBeaver / `tbm db connect` / web SQL。rotate 可。
--   どちらも同じ DB に全権。隔離しているのは「漏洩の被害面 + rotate が service を
--   切らないこと」であって権限ではない。password は復元可能な暗号化(同じパスワードで
--   再作成して復元 — v2 §11)。
-- ===========================================================================

CREATE TABLE database_roles (
    resource_id  UUID NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    role_kind    TEXT NOT NULL CHECK (role_kind IN ('app','human')),
    pg_role      TEXT NOT NULL UNIQUE,
    password_enc BYTEA NOT NULL,                        -- XChaCha20-Poly1305(nonce ‖ ciphertext)
    conn_limit   INT  NOT NULL DEFAULT 20,
    PRIMARY KEY (resource_id, role_kind)
);

-- ===========================================================================
-- audit_log(ガバナンス可視性の片割れ。owner の代理操作・rotate・削除・復元を記録)
-- ===========================================================================

CREATE TABLE audit_log (
    id              BIGINT      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_id        UUID        REFERENCES users(id),
    action          TEXT        NOT NULL,               -- 'db.create' / 'db.rotate' / 'db.delete' / 'trash.purge' …
    target_resource UUID,
    target_user     UUID,
    detail          JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX ON audit_log (created_at DESC);
CREATE INDEX ON audit_log (target_resource);
