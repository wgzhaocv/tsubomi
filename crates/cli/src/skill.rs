//! 内嵌 skill 資産 + 多 agent への投影 + self-heal。
//!
//! skill 正本(`crates/cli/skill/tsubomi-deploy.md`)は二進制に `include_str!` で
//! 埋め込み、その本文の sha256 先頭を版本戳にする。各 agent ターゲット(Claude の
//! 全局 skill / Codex の全局 AGENTS.md)へ書き出し、二進制が `tbm update` で新しく
//! なれば次回実行で戳の不一致を検出して自動で書き直す(ネットワーク不要 = 「二進制
//! だけ手動 update、skill はその投影」。自動更新の方針と整合)。uninstall は両ターゲット
//! を残留物ゼロで消す。
//!
//! ★ ターゲットを増やすときは [`target_paths`] / [`install`] / [`remove`] の 3 箇所を
//!   揃える(AGENTS.md 系は管理ブロックの挿入 / 置換 / 除去で共有ファイルを壊さない)。

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use directories::BaseDirs;
use sha2::{Digest, Sha256};

use crate::platform;

/// skill 正本(frontmatter 無しの可移植 markdown)。どの agent でも読める素の本文。
/// 本文中の `{{HOST_ARCH}}` は書き出し時に [`body_rendered`] が実プラットフォームのアーキへ置換する。
const BODY: &str = include_str!("../skill/tsubomi-deploy.md");

/// `{{HOST_ARCH}}` をこのプラットフォームのアーキ(リリース時に焼き込んだ値)へ置換した本文。
/// skill 冒頭の「このプラットフォームのアーキテクチャは … です」がこれで埋まる。
fn body_rendered() -> String {
    BODY.replace("{{HOST_ARCH}}", platform::host_arch())
}

/// Claude skill の frontmatter。`description` が skill 発火の判断材料になる。
const CLAUDE_FRONTMATTER: &str = "---\nname: tsubomi-deploy\ndescription: tsubomi(蕾)社内 PaaS を tbm CLI で扱うときの運用手順書(デプロイに限らない)。tsubomi / tbm / 蕾 が関わる作業はすべてこれに従う — service/database/volume/cache の作成・注入・デプロイ・検証、`tbm` 各コマンド(service status/logs/exec、db/cache/volume、inject、rotate、deploy --local)、GitHub 経路と退路、デプロイ可否の判断。次の症状でも必ず読む — app が `succeeded` なのに 502 / サイトが開かない、`tbm` が `unauthorized`・`conflict`・`validation` を返す、注入が効かない、rotate 後に反映されない、gh / docker が無い、GitHub Actions の枠が切れた。「tbm でデプロイ」「tsubomi にあげる」「蕾にデプロイ」等の依頼でも起動。\n---\n";

/// Codex の全局 AGENTS.md に挿す管理ブロックの目印(uninstall がこれを目当てに除去)。
const MARKER_BEGIN: &str = "<!-- >>> tbm skill: tsubomi-deploy (managed; do not edit) >>> -->";
const MARKER_END: &str = "<!-- <<< tbm skill: tsubomi-deploy <<< -->";

/// 版本戳 = 本文 + アーキ + frontmatter + マーカーの sha256 先頭 12 hex。これら **書き出す
/// 素材すべて** を含めるのが要点:どれか 1 つでも変われば戳が動き、self-heal が投影を書き直す。
/// 特に frontmatter(description = skill 発火のトリガ)を含めないと、本文 BODY が同一のまま
/// description だけ変えたときに戳が動かず、毎コマンドの self-heal が変更を取りこぼす(投影が
/// 古い description のまま残る)。素材はすべて `&str` 定数(BODY も render せず直接)なので、
/// 毎コマンド走る `ensure_fresh()` に `String` 確保(~10KB の replace)を持ち込まない。
fn hash() -> String {
    let mut h = Sha256::new();
    // 区切り(b"\0")で各素材の連結の曖昧さを断つ。
    h.update(BODY.as_bytes());
    h.update(b"\0");
    h.update(platform::host_arch().as_bytes());
    h.update(b"\0");
    h.update(CLAUDE_FRONTMATTER.as_bytes());
    h.update(b"\0");
    h.update(MARKER_BEGIN.as_bytes());
    h.update(b"\0");
    h.update(MARKER_END.as_bytes());
    hex::encode(h.finalize())[..12].to_string()
}

/// 書き出したファイルに残す戳行。self-heal はこの行の有無で「最新か」を判定する。
fn stamp_line() -> String {
    format!("<!-- tbm-skill-hash: {} -->", hash())
}

fn home() -> Option<PathBuf> {
    Some(BaseDirs::new()?.home_dir().to_path_buf())
}

/// `~/.claude/skills/tsubomi-deploy/SKILL.md`(主ターゲット。self-heal はここの戳を見る)。
fn claude_path() -> Option<PathBuf> {
    Some(home()?.join(".claude/skills/tsubomi-deploy/SKILL.md"))
}

/// `~/.codex/AGENTS.md`(Codex の全局指令。管理ブロックを挿す)。
fn codex_path() -> Option<PathBuf> {
    Some(home()?.join(".codex/AGENTS.md"))
}

/// `tbm skill print` 用:内嵌の本文(プレースホルダ置換済み)。
pub fn body() -> String {
    body_rendered()
}

/// `tbm skill where` 用:書き出し先の一覧。
pub fn target_paths() -> Vec<PathBuf> {
    [claude_path(), codex_path()].into_iter().flatten().collect()
}

/// nudge 表示用(主ターゲットのパス)。
pub fn claude_skill_path() -> Option<PathBuf> {
    claude_path()
}

/// Claude 用の完整内容(frontmatter + 戳 + 本文)。
fn claude_contents() -> String {
    format!("{CLAUDE_FRONTMATTER}{}\n\n{}", stamp_line(), body_rendered())
}

/// Codex AGENTS.md に挿す管理ブロック(戳込み)。
fn codex_block() -> String {
    format!(
        "{MARKER_BEGIN}\n{}\n\n{}\n{MARKER_END}\n",
        stamp_line(),
        body_rendered()
    )
}

/// 全ターゲットへ書き出す(既存は上書き / 置換)。書けたパスを返す。
pub fn install() -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    if let Some(p) = claude_path() {
        write_claude(&p)?;
        written.push(p);
    }
    if let Some(p) = codex_path() {
        write_codex_block(&p)?;
        written.push(p);
    }
    if written.is_empty() {
        bail!("ホームディレクトリを解決できませんでした(skill を書き出せません)");
    }
    Ok(written)
}

fn write_claude(path: &PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, claude_contents())
        .with_context(|| format!("failed to write {}", path.display()))
}

/// AGENTS.md は他の内容と共有しうるので、管理ブロックだけを挿入 / 置換する。
fn write_codex_block(path: &PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let existing = fs::read_to_string(path).unwrap_or_default();
    let next = replace_or_append_block(&existing, &codex_block());
    fs::write(path, next).with_context(|| format!("failed to write {}", path.display()))
}

/// 既存テキストの管理ブロックを差し替える。無ければ末尾に足す(前に空行 1 つ)。
fn replace_or_append_block(existing: &str, block: &str) -> String {
    if let (Some(b), Some(e)) = (existing.find(MARKER_BEGIN), existing.find(MARKER_END))
        && b < e + MARKER_END.len()
    {
        let end = e + MARKER_END.len();
        let mut out = String::with_capacity(existing.len());
        out.push_str(&existing[..b]);
        out.push_str(block.trim_end());
        out.push_str(&existing[end..]);
        return out;
    }
    if existing.trim().is_empty() {
        block.to_string()
    } else {
        format!("{}\n\n{block}", existing.trim_end())
    }
}

/// self-heal:全ターゲット(Claude SKILL.md / Codex AGENTS.md 管理ブロック)のどれかが
/// 無い / 戳が古ければ全ターゲットを書き直す。書いたら `true`。主ターゲットだけ見ると
/// Claude が最新でも Codex 側のブロックが消え / 古いまま取りこぼされるため、全ターゲットを見る。
/// 失敗(権限 / HOME 不明)は黙って `false` — skill の管理で通常コマンドを妨げない。
pub fn ensure_fresh() -> bool {
    let targets = target_paths();
    if targets.is_empty() {
        return false; // HOME 不明など — 通常コマンドを妨げない。
    }
    // stamp_line() は hash() を計算するので一度だけ取り、全ターゲットで使い回す。
    let stamp = stamp_line();
    let all_fresh = targets
        .iter()
        .all(|p| fs::read_to_string(p).ok().is_some_and(|c| c.contains(&stamp)));
    if all_fresh {
        return false;
    }
    install().is_ok()
}

/// uninstall:両ターゲットを残留物ゼロで消す。Claude = skill ディレクトリごと、
/// Codex = 管理ブロックのみ除去(他の内容は残す。空になればファイルも消す)。best-effort。
pub fn remove() {
    if let Some(p) = claude_path()
        && let Some(dir) = p.parent()
    {
        let _ = fs::remove_dir_all(dir);
    }
    if let Some(p) = codex_path()
        && let Ok(existing) = fs::read_to_string(&p)
        && existing.contains(MARKER_BEGIN)
    {
        let stripped = strip_block(&existing);
        if stripped.trim().is_empty() {
            let _ = fs::remove_file(&p);
        } else {
            let _ = fs::write(&p, stripped);
        }
    }
}

/// 管理ブロックを取り除く(前後の余分な空白は整える)。マーカーが無ければそのまま。
fn strip_block(existing: &str) -> String {
    let (Some(b), Some(e)) = (existing.find(MARKER_BEGIN), existing.find(MARKER_END)) else {
        return existing.to_string();
    };
    let end = e + MARKER_END.len();
    if b >= end {
        return existing.to_string();
    }
    let before = existing[..b].trim_end();
    let after = existing[end..].trim_start_matches('\n');
    if before.is_empty() {
        return after.to_string();
    }
    if after.is_empty() {
        return format!("{before}\n");
    }
    format!("{before}\n\n{after}")
}
