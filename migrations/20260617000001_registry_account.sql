-- tsubomi M3 S4:per-user の registry 資格情報。
--
-- ユーザ app のイメージ push 先(GitHub Actions が docker login する)registry の
-- アカウントを **ユーザ単位で 1 つ**持つ(per-service ではない — digest ピン留めで
-- per-repo ACL が不要。決定 #3 / paas-m3-design §11-D)。同じユーザの複数 service が
-- 同じ資格情報を共有する(create のたびに同じ creds を GitHub Secret へ書く)。
--
-- 平台は service create のレスポンスでこの password の **原文**を GitHub Secret 用に
-- 返す必要があるので、ハッシュ(session / cli_token)ではなく **復元可能な暗号化**で
-- 持つ(crypto.rs、XChaCha20-Poly1305。tech-design §7 の資格情報分立)。
--
-- registry の htpasswd ファイルそのものへの同期(bcrypt 行の追記 + registry への
-- SIGHUP リロード)は **prod-infra スライス**で足す:認証付き registry が立ってから
-- 実機検証する(dev の registry は認証なし)。本テーブルはアカウントの永続化と
-- creds 返却までを担う。

CREATE TABLE registry_accounts (
    user_id      UUID        PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    username     TEXT        NOT NULL UNIQUE,           -- docker login / registry のユーザ名(u-<uuid>)
    password_enc BYTEA       NOT NULL,                  -- 原文の at-rest 暗号化(nonce ‖ ct)
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
