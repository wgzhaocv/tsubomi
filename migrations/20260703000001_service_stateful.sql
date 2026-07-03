-- service の deploy 語義:false = start-first swap(無瞬断・無状態前提 = 現状)/
-- true = stop-first(数秒瞬断・データ目録の単独占有。自帯 DB 等の有状態コンテナ用)。
-- swap は新旧容器が同一データ目録を同時に開く(postgres の postmaster.pid 防双開は
-- 跨 PID namespace で信頼できない → 双開 = データ破壊)ため、有状態には stop-first が必須。
-- 既存行は DEFAULT false = 挙動不変。設計は doc/paas-service-stateful-design.md。
ALTER TABLE service_details
  ADD COLUMN stateful BOOLEAN NOT NULL DEFAULT false;
