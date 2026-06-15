-- M4 S5:共有パスワード viewer(design v2 §7「見るは共有密码」)。
-- session 単位の閲覧 grant。viewer/login 成功で now()+8h を入れ、> now() の間だけ
-- 管制面(overview / ranking)を只读で見られる。owner の共有パスワード reset で
-- 全 grant を NULL に戻す = 旧パスワードは即失効。
-- 共有パスワード本体は platform_config['viewer_password'] = {hash, updated_at, updated_by}。
ALTER TABLE sessions ADD COLUMN viewer_until TIMESTAMPTZ;
