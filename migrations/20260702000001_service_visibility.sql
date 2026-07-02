-- service 公開範囲(visibility)三態。traefik route ファイル(svc-<id>.yml)の生成を分岐する:
--   private = route を書かない(公網不可視。subdomain は温存し、再公開で同じ URL が復活)
--   company = 既定(route + 会社 IP 許可リスト middleware)= 従来挙動
--   public  = route はあるが ipallow middleware を挂けない(全網公開)
-- 既存行は DEFAULT で全部 company = 挙動不変。
ALTER TABLE service_details
  ADD COLUMN visibility TEXT NOT NULL DEFAULT 'company'
  CHECK (visibility IN ('private','company','public'));
