//! `tbm skill`:AI エージェント向けデプロイ skill の管理。
//!
//! skill 正本は二進制に内嵌され、普段は毎回の self-heal(`crate::skill::ensure_fresh`)が
//! 旧 / 欠けのときだけ自動で書き出す。ここの `install` は**強制再書き出し**(インストーラや
//! 手動の復旧用)。owner 専用ではなく全ユーザ向け — デプロイは AI 駆動の通常操作だから。

use anyhow::Result;
use clap::Subcommand;

use crate::skill;

#[derive(Subcommand)]
pub enum SkillCmd {
    /// 全 agent ターゲット(Claude + 共有技能庫 ~/.agents/skills + 各 agent の symlink)へ
    /// skill を書き出す / 張り直す(既存は上書き。旧 Codex AGENTS.md ブロックも掃除)
    Install,
    /// 書き出し先のパスを表示
    Where,
    /// skill 本文を stdout に出力(確認 / パイプ用)
    Print,
}

pub async fn run(action: SkillCmd) -> Result<()> {
    match action {
        SkillCmd::Install => {
            for p in skill::install()? {
                println!("{}", p.display());
            }
            // 投影できなかったターゲット(権限で symlink / 実複写できない agent 等)を可視化。
            let stale = skill::stale_targets();
            if !stale.is_empty() {
                eprintln!("warning: 次のターゲットへは投影できませんでした(権限などを確認):");
                for p in stale {
                    eprintln!("  {}", p.display());
                }
            }
        }
        SkillCmd::Where => {
            for p in skill::target_paths() {
                println!("{}", p.display());
            }
        }
        SkillCmd::Print => print!("{}", skill::body()),
    }
    Ok(())
}
