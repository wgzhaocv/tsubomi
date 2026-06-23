-- tsubomi M3:service の detail + 静的 env + 注入 + デプロイ。
-- 背骨(tech-design §2):4 種のリソースは resources スーパーテーブル 1 枚 +
-- 種別毎の detail テーブル。M3 では service の detail に加え、env / injections /
-- deploys / deploy_nonces を足す(§9 の規律:env と注入は「注入の相手 = service」が
-- 在って初めて意味を持つので、このフェーズで入る)。
-- 実装手順の詳細は doc/paas-m3-design.md。

-- ===========================================================================
-- service_details(resources と 1:1)
--   subdomain は <service[-乱数語]>.<ドメイン> の左辺(グローバル一意)。
--   deploy_key は HMAC の鍵原文 — 平台が原文を必要とするので復元可能に暗号化する
--   (session / cli_token のようなハッシュではない。tech-design §7 の資格情報分立)。
--   image_digest は「現在走るべきイメージ」(tag ではなく content-addressed digest。決定 #3)。
-- ===========================================================================

CREATE TABLE service_details (
    resource_id    UUID        PRIMARY KEY,
    kind           TEXT        NOT NULL DEFAULT 'service' CHECK (kind = 'service'),
    repo           TEXT,                                  -- "owner/name"。ユーザ自身の gh で作成(S4)
    subdomain      TEXT        NOT NULL UNIQUE,           -- <service[-乱数語]>。ドメインは付けない
    deploy_key_enc BYTEA       NOT NULL,                  -- HMAC の鍵原文。XChaCha20-Poly1305(nonce ‖ ct)
    image_digest   TEXT,                                  -- 現在走るべきイメージ(sha256:…、決定 #3)
    desired_state  TEXT        NOT NULL DEFAULT 'stopped'
                   CHECK (desired_state IN ('running','stopped')),
    phase          TEXT        NOT NULL DEFAULT 'created'
                   CHECK (phase IN ('created','deploying','running','stopped','failed')),
    phase_detail   TEXT,                                  -- 失敗理由など(UI/CLI 向け)
    memory_mb      INT         NOT NULL DEFAULT 512,      -- --memory 硬上限(OOM は単一コンテナだけ殺す)
    cpu_shares     INT         NOT NULL DEFAULT 1024,     -- --cpu-shares ソフト制限
    container_port INT         NOT NULL DEFAULT 8080,     -- app が容器内で listen する port(traefik の転送先)
    public         BOOLEAN     NOT NULL DEFAULT false,    -- true = ipAllowList から除外
    compose_spec   JSONB,                                 -- null=単一コンテナ;複数コンテナは M6
    last_deploy_at TIMESTAMPTZ,
    FOREIGN KEY (resource_id, kind) REFERENCES resources(id, kind) ON DELETE CASCADE
);

-- ===========================================================================
-- service_env(静的 env:人 / AI が入れたリテラル値。値は暗号化)
-- ===========================================================================

CREATE TABLE service_env (
    service_id UUID  NOT NULL,
    skind      TEXT  NOT NULL DEFAULT 'service' CHECK (skind = 'service'),
    key        TEXT  NOT NULL,
    value_enc  BYTEA NOT NULL,                            -- XChaCha20-Poly1305(nonce ‖ ct)
    PRIMARY KEY (service_id, key),
    FOREIGN KEY (service_id, skind) REFERENCES resources(id, kind) ON DELETE CASCADE
);

-- ===========================================================================
-- injections(「バインディング」だけを保存。値はコンテナ起動の瞬間に都度解決 — 決定 #5)
--   ソフト削除(resources.deleted_at)はこの表に触れない ⇒ 注入は宙吊りで失効し、
--   復元すれば自動的に生き返る(v2 §11 の意味論がタダで成立)。物理削除(purge)は
--   resource_id の FK カスケードでバインディングを掃除する。
-- ===========================================================================

CREATE TABLE injections (
    id          UUID  PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id  UUID  NOT NULL,
    skind       TEXT  NOT NULL DEFAULT 'service' CHECK (skind = 'service'),
    resource_id UUID  NOT NULL REFERENCES resources(id) ON DELETE CASCADE,  -- 注入元(database / volume / cache)
    env_var     TEXT  NOT NULL,                           -- DATABASE_URL / REDIS_URL / STORAGE_PATH …
    mount_path  TEXT,                                     -- volume のみ:コンテナ内マウント先(既定 /data/<名>)
    UNIQUE (service_id, env_var),
    FOREIGN KEY (service_id, skind) REFERENCES resources(id, kind) ON DELETE CASCADE
);

-- ===========================================================================
-- deploys(デプロイ履歴。rollback は履歴の digest を選んで再起動する — m3-design §6.8)
-- ===========================================================================

CREATE TABLE deploys (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id   UUID        NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    git_sha      TEXT        NOT NULL,
    image_digest TEXT        NOT NULL,
    status       TEXT        NOT NULL
                 CHECK (status IN ('received','pulling','starting','succeeded','failed')),
    error        TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at  TIMESTAMPTZ
);

CREATE INDEX ON deploys (service_id, created_at DESC);

-- ===========================================================================
-- deploy_nonces(hook のリプレイ防御:ts ± 300s + nonce 一意。reconcile が 1h 超を掃除)
-- ===========================================================================

CREATE TABLE deploy_nonces (
    service_id UUID        NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    nonce      TEXT        NOT NULL,
    seen_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (service_id, nonce)
);
