-- 監査ログに client IP を残す(CF Tunnel 越しの実 client IP = CF-Connecting-IP ヘッダ)。
-- 既存行・background 操作(actor=None)・ヘッダ不在は NULL。TEXT(INET でなく)で持つ —
-- 不正な値でも INSERT を失敗させず best-effort の監査を壊さないため。
ALTER TABLE audit_log ADD COLUMN client_ip TEXT;
