-- 直前の migration(20260726000001)が既存行を **'-infinity'** で埋めたのを修正する。
-- Postgres の infinity は `DateTime<Utc>` に読み込めず、sqlx が
-- 「`NaiveDateTime + TimeDelta` overflowed」で **panic** する(注入一覧の端点が落ちる)。
-- 本番で実測:既存行を持つ service の `GET /injections` が全て panic した(dev は既存行が
-- 無かったので露出しなかった = 「既存データだけが踏む」型の穴)。
--
-- 意図は変えない(「昔から在る = 反映済み扱い」)ので、**表現可能な下限**である epoch に寄せる。
-- 平台の運用開始は 2026 年なので、epoch より前の deploy は存在し得ず判定は同じ。
-- 適用済み migration は不変なので、前のファイルは直さずここで上書きする(CLAUDE.md の約束)。
UPDATE injections SET created_at = 'epoch' WHERE created_at = '-infinity';
