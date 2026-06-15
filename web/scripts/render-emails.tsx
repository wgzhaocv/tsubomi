// React Email テンプレを静的 HTML に焼き、Rust が include_str! で埋め込む場所
// (crates/server/src/mail/templates/*.html)へ書き出す。動的箇所は {{VAR}} の
// プレースホルダのまま出力し、Rust 側(mail::render)が HTML エスケープした値で置換する。
//
// 実行:`just emails`(= bun run web/scripts/render-emails.tsx)。テンプレ(.tsx)を
// 変えたら必ず再実行して生成 HTML をコミットする(生成物だが include_str! の都合で commit)。
import { render } from "@react-email/render";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

import ActionCode from "../emails/templates/ActionCode";
import DiskAlert from "../emails/templates/DiskAlert";
import OwnerRemove from "../emails/templates/OwnerRemove";

const OUT = join(import.meta.dirname, "../../crates/server/src/mail/templates");

const templates = [
  { file: "owner_remove.html", el: <OwnerRemove /> },
  { file: "disk_alert.html", el: <DiskAlert /> },
  { file: "action_code.html", el: <ActionCode /> },
];

await mkdir(OUT, { recursive: true });
for (const t of templates) {
  const html = await render(t.el, { pretty: true });
  await writeFile(join(OUT, t.file), html, "utf8");
  console.log(`✓ ${t.file} (${html.length} bytes)`);
}
console.log(`rendered ${templates.length} email templates → ${OUT}`);
