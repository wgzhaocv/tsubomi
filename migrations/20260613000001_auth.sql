-- tsubomi M0:アイデンティティ + 認証テーブル。
-- 構造は amber の init マイグレーションからの移植(users + credentials 分割、
-- passkey 対応準備済み)。tsubomi の差分:role カラム(owner)、email NOT NULL
-- (hd 制限付き Google は必ず email を返す)、session / oauth state を
-- Redis ではなく Postgres に置く(valkey が入るのは M5)。

CREATE TYPE credential_type AS ENUM ('google', 'passkey');
CREATE TYPE user_role AS ENUM ('user', 'owner');

-- ===========================================================================
-- users
-- ===========================================================================

CREATE TABLE users (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    email         TEXT        UNIQUE NOT NULL,
    name          TEXT,
    avatar_url    TEXT,
    role          user_role   NOT NULL DEFAULT 'user',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_login_at TIMESTAMPTZ
);

-- ===========================================================================
-- credentials(今は Google OAuth、後で WebAuthn passkey — amber と同じ形)
-- ===========================================================================

CREATE TABLE credentials (
    id          UUID            PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID            NOT NULL REFERENCES users(id),
    type        credential_type NOT NULL,
    external_id TEXT            NOT NULL,
    public_key  BYTEA,
    counter     BIGINT,
    created_at  TIMESTAMPTZ     NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ     NOT NULL DEFAULT now(),
    CONSTRAINT passkey_fields_check CHECK (
        (type = 'passkey' AND public_key IS NOT NULL AND counter IS NOT NULL)
        OR
        (type = 'google'  AND public_key IS NULL     AND counter IS NULL)
    )
);

CREATE INDEX        ON credentials (user_id, type);
CREATE UNIQUE INDEX ON credentials (type, external_id);

-- ===========================================================================
-- sessions(Web ログイン状態。cookie は生トークン、DB は sha256 を保持)
-- ===========================================================================

CREATE TABLE sessions (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash TEXT        NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX ON sessions (expires_at);

-- ===========================================================================
-- cli_tokens(tbm / AI 用の Bearer。平文は一度だけ表示、保存は sha256)
-- ===========================================================================

CREATE TABLE cli_tokens (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT        NOT NULL,
    token_hash   TEXT        NOT NULL UNIQUE,
    expires_at   TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    revoked_at   TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ===========================================================================
-- oauth_states(Google ログインの CSRF state。DELETE..RETURNING による単回
-- 消費 — amber の Redis GETDEL の Postgres 等価)
-- ===========================================================================

CREATE TABLE oauth_states (
    state      TEXT        PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);

-- ===========================================================================
-- authcodes(tbm ログインの保留中 PKCE コード。単回使用・TTL 10 分。
-- 生のまま保存:verifier を持たない者には漏れた保留コードも無価値 —
-- PKCE S256 はまさにこのための防御)
-- ===========================================================================

CREATE TABLE authcodes (
    code           TEXT        PRIMARY KEY,
    user_id        UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_challenge TEXT        NOT NULL,
    state          TEXT        NOT NULL,
    hint           TEXT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at     TIMESTAMPTZ NOT NULL
);
