-- deploys に commit message(`git log -1 --pretty=%s` の件名)を足す。
-- 既存行・message を送らない旧 workflow からの hook は NULL(前端は git_sha に回退)。
ALTER TABLE deploys ADD COLUMN commit_message TEXT;
