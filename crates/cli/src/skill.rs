//! 内嵌 skill 資産 + 多 agent への投影 + self-heal。
//!
//! skill 正本(`crates/cli/skill/tsubomi-deploy.md`)は二進制に `include_str!` で
//! 埋め込み、その本文の sha256 先頭を版本戳にする。各 agent ターゲットへ書き出し、
//! 二進制が `tbm update` で新しくなれば次回実行で戳の不一致を検出して自動で書き直す
//! (ネットワーク不要 = 「二進制だけ手動 update、skill はその投影」。自動更新の方針と整合)。
//!
//! ターゲットは 2 系統:
//!   1. **Claude**:`~/.claude/skills/tsubomi-deploy/SKILL.md`(実ファイル。self-heal の主 anchor)。
//!   2. **その他の一切の agent**:共有技能庫 `~/.agents/skills/tsubomi-deploy/SKILL.md` を正本に
//!      書き、各 agent(`~/.codex`・`~/.gemini`・…)の `skills/` からそこへ **symlink** を張る。
//!      これがこのマシンの多 agent 共有の流儀(find-skills 等も全部このレイアウト)。以前は
//!      Codex 私有の `~/.codex/AGENTS.md` に管理ブロックを挿していたが、それでは Codex 一体しか
//!      届かないので廃止し、[`migrate_legacy`] が旧ブロックを掃除する(他機の `tbm update` 後、
//!      次コマンドの self-heal で自動除去)。
//!
//! ★ 対応 agent を増やすときは [`AGENT_SKILL_DIRS`] に 1 行足すだけ(install / where /
//!   self-heal / uninstall は全部この一覧から回る)。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use directories::BaseDirs;
use sha2::{Digest, Sha256};

use crate::platform;

/// skill 正本(frontmatter 無しの可移植 markdown)。どの agent でも読める素の本文。
/// 本文中の `{{HOST_ARCH}}` は書き出し時に [`body_rendered`] が実プラットフォームのアーキへ置換する。
const BODY: &str = include_str!("../skill/tsubomi-deploy.md");

/// skill のフォルダ名(共有技能庫の目録名 + 各 agent skills 下の symlink 名)。
const SKILL_NAME: &str = "tsubomi-deploy";

/// `{{HOST_ARCH}}` をこのプラットフォームのアーキ(リリース時に焼き込んだ値)へ置換した本文。
/// skill 冒頭の「このプラットフォームのアーキテクチャは … です」がこれで埋まる。
fn body_rendered() -> String {
    BODY.replace("{{HOST_ARCH}}", platform::host_arch())
}

/// SKILL.md の frontmatter。`description` が skill 発火の判断材料になる。Claude も共有技能庫も
/// この同一形式(frontmatter + 本文)の SKILL.md を読むので、両者で使い回す。
const SKILL_FRONTMATTER: &str = "---\nname: tsubomi-deploy\ndescription: tsubomi(蕾)社内 PaaS を tbm CLI で扱うときの運用手順書(デプロイに限らない)。tsubomi / tbm / 蕾 が関わる作業はすべてこれに従う — service/database/volume/cache の作成・注入・デプロイ・検証、`tbm` 各コマンド(service status/logs/exec、db/cache/volume、inject、rotate、deploy --local/--image/--dockerfile)、GitHub 経路と退路(既成イメージ・無 context Dockerfile はサーバ側取得/ビルド = docker 不要)、デプロイ可否の判断。次の症状でも必ず読む — app が `succeeded` なのに 502 / サイトが開かない、`tbm` が `unauthorized`・`conflict`・`validation` を返す、注入が効かない、rotate 後に反映されない、gh / docker が無い、GitHub Actions の枠が切れた。「tbm でデプロイ」「tsubomi にあげる」「蕾にデプロイ」等の依頼でも起動。\n---\n";

/// 旧 `~/.codex/AGENTS.md` 管理ブロックの目印([`migrate_legacy`] がこれを目当てに除去)。
/// 新規書き出しには使わない — 旧版の残骸掃除専用。
const MARKER_BEGIN: &str = "<!-- >>> tbm skill: tsubomi-deploy (managed; do not edit) >>> -->";
const MARKER_END: &str = "<!-- <<< tbm skill: tsubomi-deploy <<< -->";

/// このマシンで tsubomi skill を投影する非 Claude agent の `skills/` 目録(home 相対)。
/// **各 agent が実際にインストール済みのときだけ**(= `skills/` の親目録が存在するとき)投影する。
/// 全部この共有技能庫 `~/.agents/skills/tsubomi-deploy` への symlink になる。
const AGENT_SKILL_DIRS: &[&str] = &[
    ".codex/skills",
    ".gemini/skills",
    ".qwen/skills",
    ".factory/skills",
    ".continue/skills",
    ".config/goose/skills",
    ".config/opencode/skills",
];

/// 版本戳 = 本文 + アーキ + frontmatter + 名前の sha256 先頭 12 hex。これら **書き出す
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
    h.update(SKILL_FRONTMATTER.as_bytes());
    h.update(b"\0");
    h.update(SKILL_NAME.as_bytes());
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
    Some(home()?.join(".claude/skills").join(SKILL_NAME).join("SKILL.md"))
}

/// 共有技能庫の正本 `~/.agents/skills/tsubomi-deploy/SKILL.md`(非 Claude agent の symlink 先)。
fn store_dir() -> Option<PathBuf> {
    Some(home()?.join(".agents/skills").join(SKILL_NAME))
}
fn store_path() -> Option<PathBuf> {
    Some(store_dir()?.join("SKILL.md"))
}

/// 旧 `~/.codex/AGENTS.md`(かつての Codex 専用ターゲット。今は掃除対象)。
fn legacy_codex_agents_md() -> Option<PathBuf> {
    Some(home()?.join(".codex/AGENTS.md"))
}

/// インストール済み agent の symlink パス一覧(`<agent>/skills/tsubomi-deploy`)。
/// `skills/` の親(= agent 本体の目録)が存在するものだけ = ユーザが実際に使う agent だけに投影。
fn present_agent_links() -> Vec<PathBuf> {
    let Some(home) = home() else {
        return Vec::new();
    };
    AGENT_SKILL_DIRS
        .iter()
        .map(|rel| home.join(rel))
        .filter(|skills_dir| skills_dir.parent().is_some_and(|base| base.exists()))
        .map(|skills_dir| skills_dir.join(SKILL_NAME))
        .collect()
}

/// `tbm skill print` 用:内嵌の本文(プレースホルダ置換済み)。
pub fn body() -> String {
    body_rendered()
}

/// `tbm skill where` 用 + self-heal の鮮度判定用:管理下の **SKILL.md ファイル** の一覧。
/// Claude 実ファイル + 共有技能庫の正本 + 各 agent の symlink 越しの `SKILL.md`。agent 側は
/// symlink が指すのは *目録* なので、鮮度判定(戳の read)には中の `SKILL.md` を指す必要がある
/// (symlink 目録そのものを read_to_string すると必ず失敗し、毎回「陳腐」= 毎コマンド再投影になる)。
pub fn target_paths() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = [claude_path(), store_path()].into_iter().flatten().collect();
    v.extend(present_agent_links().into_iter().map(|l| l.join("SKILL.md")));
    v
}

/// nudge 表示用(主ターゲットのパス)。
pub fn claude_skill_path() -> Option<PathBuf> {
    claude_path()
}

/// SKILL.md の完整内容(frontmatter + 戳 + 本文)。Claude / 共有技能庫の両方でこれを書く。
fn skill_md_contents() -> String {
    format!("{SKILL_FRONTMATTER}{}\n\n{}", stamp_line(), body_rendered())
}

/// 全ターゲットへ書き出す(既存は上書き / 張り直し)。書けた / 張れたパスを返す。
pub fn install() -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();

    // 1. Claude:実ファイル(主 anchor)。
    if let Some(p) = claude_path() {
        write_skill_md(&p)?;
        written.push(p);
    }

    // 2. 共有技能庫の正本。
    if let Some(p) = store_path() {
        write_skill_md(&p)?;
        written.push(p.clone());
        // 3. 各 agent から正本へ symlink(best-effort — 権限 / 非対応 FS では黙って飛ばす)。
        if let Some(home) = home() {
            for link in present_agent_links() {
                if link_to_store(&link, &home).is_ok() {
                    written.push(link);
                }
            }
        }
    }

    // 4. 旧 Codex AGENTS.md ブロックの掃除(移行)。
    migrate_legacy();

    if written.is_empty() {
        bail!("ホームディレクトリを解決できませんでした(skill を書き出せません)");
    }
    Ok(written)
}

/// パスの中身が最新戳を含むか(symlink 越しでも中身を読む)。
fn is_fresh(path: &Path, stamp: &str) -> bool {
    fs::read_to_string(path)
        .ok()
        .is_some_and(|c| c.contains(stamp))
}

/// SKILL.md を実ファイルとして書く(親目録が無ければ作る)。既に最新なら書かない
/// (install() が毎回呼ばれても Claude / 正本を無駄に上書きしないため)。
fn write_skill_md(path: &Path) -> Result<()> {
    if is_fresh(path, &stamp_line()) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, skill_md_contents())
        .with_context(|| format!("failed to write {}", path.display()))
}

/// `<agent>/skills/tsubomi-deploy` → 共有技能庫の正本目録へ symlink を張る。既存の実体
/// (古い symlink / 実目録)があれば剥がしてから張り直す。symlink 不可の環境(Windows で
/// 権限無し等)では正本を実目録として複写して代替する。
fn link_to_store(link: &Path, home: &Path) -> Result<()> {
    // 既に正本の最新 SKILL.md を指しているなら張り直さない(毎回の remove+symlink を避ける)。
    if is_fresh(&link.join("SKILL.md"), &stamp_line()) {
        return Ok(());
    }
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    remove_any(link);
    let target = relative_store_target(link, home);
    if make_symlink(&target, link).is_ok() {
        return Ok(());
    }
    // 代替:実目録に SKILL.md を複写(symlink 非対応環境)。
    fs::create_dir_all(link).with_context(|| format!("failed to create {}", link.display()))?;
    fs::write(link.join("SKILL.md"), skill_md_contents())
        .with_context(|| format!("failed to write {}", link.join("SKILL.md").display()))
}

/// symlink の相対ターゲット。link は `<home>/<…>/skills/tsubomi-deploy`。link の親
/// (`skills/`)から home まで戻り、`.agents/skills/tsubomi-deploy` へ下る。既存の
/// マシン上の symlink(find-skills 等)と同じ相対形にする。
fn relative_store_target(link: &Path, home: &Path) -> PathBuf {
    // 親 = <agent>/skills。home からの深さ分だけ `..` を積む。
    let depth = link
        .parent()
        .and_then(|p| p.strip_prefix(home).ok())
        .map(|rel| rel.components().count())
        .unwrap_or(0);
    let mut t = PathBuf::new();
    for _ in 0..depth {
        t.push("..");
    }
    t.push(".agents/skills");
    t.push(SKILL_NAME);
    t
}

#[cfg(unix)]
fn make_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}
#[cfg(windows)]
fn make_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}
#[cfg(not(any(unix, windows)))]
fn make_symlink(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("symlink unsupported on this platform"))
}

/// パスにある何か(symlink / 実ファイル / 実目録)を best-effort で消す。symlink は
/// 指す先が目録でも `remove_file` で外れる(リンク自体だけ消える)。
fn remove_any(path: &Path) {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return; // 無ければ何もしない。
    };
    if meta.file_type().is_symlink() || meta.is_file() {
        let _ = fs::remove_file(path);
    } else if meta.is_dir() {
        let _ = fs::remove_dir_all(path);
    }
}

/// self-heal:全ターゲット(Claude / 共有技能庫 / インストール済み agent の symlink)のどれかが
/// 無い / 戳が古ければ全ターゲットを書き直す。書いたら `true`。symlink 越しの read は正本の
/// 中身(戳入り)を返すので鮮度判定はそのまま効く。失敗(権限 / HOME 不明)は黙って `false`
/// — skill の管理で通常コマンドを妨げない。
pub fn ensure_fresh() -> bool {
    // 旧 AGENTS.md の掃除は鮮度に関係なく毎回試す(新ターゲットが既に最新でも旧残骸を残さない)。
    migrate_legacy();

    // stamp_line() は hash() を計算するので一度だけ取り、全判定で使い回す。
    let stamp = stamp_line();

    // 必須ターゲット = Claude(主 anchor)+ 共有技能庫の正本。ここが古い / 欠けたら「版が動いた」。
    let required: Vec<PathBuf> = [claude_path(), store_path()].into_iter().flatten().collect();
    if required.is_empty() {
        return false; // HOME 不明など — 通常コマンドを妨げない。
    }
    let required_fresh = required.iter().all(|p| is_fresh(p, &stamp));

    // agent 側 symlink は best-effort の投影。欠け / 古ければ張り直すが、投影に失敗し続ける環境
    // (権限で skills/ を作れない等)で毎コマンド install()+nudge を撃たないよう、鮮度判定は
    // ここに含めても **nudge(戻り値)には数えない**。
    let links_stale = present_agent_links()
        .into_iter()
        .any(|l| !is_fresh(&l.join("SKILL.md"), &stamp));

    if required_fresh && !links_stale {
        return false; // 何もすることが無い。
    }
    // 全ターゲットを収束(既に最新のものは write/link 側でスキップ = 廉価)。
    let _ = install();
    // nudge は「必須が古かった = 内容が変わった」ときだけ。symlink 張り直しだけなら黙る。
    !required_fresh
}

/// `tbm skill install` 用:投影できていない(最新戳を持たない)ターゲット一覧。
/// best-effort の失敗(権限で symlink / 実複写できない agent 等)を可視化する。
pub fn stale_targets() -> Vec<PathBuf> {
    let stamp = stamp_line();
    target_paths()
        .into_iter()
        .filter(|p| !is_fresh(p, &stamp))
        .collect()
}

/// uninstall:全ターゲットを残留物ゼロで消す。Claude / 共有技能庫 = 目録ごと、
/// agent 側 = symlink(or 代替の実目録)を剥がす。旧 Codex AGENTS.md ブロックも掃除。best-effort。
pub fn remove() {
    if let Some(p) = claude_path()
        && let Some(dir) = p.parent()
    {
        let _ = fs::remove_dir_all(dir);
    }
    for link in present_agent_links() {
        remove_any(&link);
    }
    if let Some(dir) = store_dir() {
        let _ = fs::remove_dir_all(dir);
    }
    migrate_legacy();
}

/// 移行:旧 `~/.codex/AGENTS.md` に残る管理ブロックを剥がす。ブロックが無ければ何もしない。
/// 剥がした結果ファイルが空になれば削除、そうでなければ他の内容を残して書き戻す。best-effort。
fn migrate_legacy() {
    let Some(p) = legacy_codex_agents_md() else {
        return;
    };
    let Ok(existing) = fs::read_to_string(&p) else {
        return; // 無ければ何もしない。
    };
    if !existing.contains(MARKER_BEGIN) {
        return; // 既に掃除済み / 元から無い。
    }
    let stripped = strip_block(&existing);
    if stripped.trim().is_empty() {
        let _ = fs::remove_file(&p);
    } else {
        let _ = fs::write(&p, stripped);
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
