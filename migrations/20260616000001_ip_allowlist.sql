-- tsubomi ガバナンス:会社 IP 許可リスト(traefik ipAllowList の管制面の正本)。
-- 背骨(tech-design §7「セキュリティの柵一覧」):会社 IP 許可リストは
-- traefik の ipAllowList middleware に住み、全 service にデフォルト適用される。
-- ここはその CIDR レンジの「期望状態」を持つ正本 — owner が編集し、平台が
-- traefik の動的設定ファイルへ収束させる(変更の度に書き直し、file provider が
-- ホットリロード)。
--
-- 意味は「許可リスト」:
--   * 空        = 制限なし(全 IP 許可、fail-open)。設定するまで誰でも繋がる。
--   * 1 件以上  = 列挙した CIDR だけが service に到達でき、他は遮断。
-- registry / deploy hook は許可リストから除外(決定 #4)— middleware を
-- 参照する label を付けないことで除外する(平台側の責務)。

-- ===========================================================================
-- ip_allow_entries:許可する CIDR レンジ(単一 IP は /32・/128 に正規化して保存)。
--   cidr は正規化済み文字列。UNIQUE で重複追加は 409 Conflict に落とす。
--   created_by は追加した owner(監査の補助。ユーザ削除に追従して NULL)。
-- ===========================================================================

CREATE TABLE ip_allow_entries (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    cidr        TEXT        NOT NULL UNIQUE,
    note        TEXT        NOT NULL DEFAULT '',
    created_by  UUID        REFERENCES users(id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX ON ip_allow_entries (created_at DESC);
