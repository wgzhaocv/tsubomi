-- service の CPU 硬上限(millicores、1000 = 1 CPU。docker の NanoCpus に変換して適用)。
-- NULL = 硬上限なし(従来どおり cpu_shares のソフト権重のみ)。メモリ(--memory)と対になる
-- 多租戸の暴走隔離(AI 審査 R4):単機で CPU を食い尽くす service が隣人を拖らせない退路。
alter table service_details add column cpu_limit_millis integer;
