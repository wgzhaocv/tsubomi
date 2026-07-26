-- 注入の作成時刻。「この注入は今動いているコンテナに反映されているか」を判定するために足す
-- (値はコンテナ起動の瞬間に解決される = 決定 #5 なので、起動より後に作られた注入は未反映)。
--
-- 既存行は **'-infinity'** で埋める:いつ作られたか分からないものを now() にすると
-- 「直近の deploy より新しい」= 全部が未反映と誤報するため、「昔から在る = 反映済み」側に倒す
-- (誤警報より見逃しの方がまし。実際その多くは既にデプロイ済み)。
ALTER TABLE injections ADD COLUMN created_at TIMESTAMPTZ;
UPDATE injections SET created_at = '-infinity' WHERE created_at IS NULL;
ALTER TABLE injections
    ALTER COLUMN created_at SET NOT NULL,
    ALTER COLUMN created_at SET DEFAULT now();
