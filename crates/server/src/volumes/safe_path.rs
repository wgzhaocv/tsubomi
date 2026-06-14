//! 假根(volume の host_path)内にユーザ/AI 由来の相対パスを必ず収める —
//! **唯一のハード安全境界**(design v2 §6 / tech-design §7)。漏らせば假根が
//! 穿透され、他人や宿主機のファイルに届く。
//!
//! 二段構え:
//!  1. `normalize_rel` — 純粋・プラットフォーム非依存。`..` / NUL / 制御文字を拒否し、
//!     先頭スラッシュ(「/ から始まる」假根の見た目)を剥がして root 相対に畳む。
//!     これだけで *文字列*トラバーサル(`../../etc/passwd`、絶対パス)を落とす。
//!  2. syscall ゲート — 既存ディレクトリ経由のシンボリックリンク越えを塞ぐ。
//!     - **Linux(本番)**:`openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)`。
//!       カーネルが「root 内・symlink 無し」を 1 syscall で断言する。
//!     - **macOS 等(dev のみ)**:`canonicalize` で実体解決し `starts_with(root)` を断言。
//!       openat2 が無いので軟い網。**本番は必ず Linux** なのでハード保証は保たれる。
//!
//! 脅威モデル:サーバは単一 uid の唯一の書き手で、テナントは物理ディレクトリで隔離
//! (他人の root に symlink を仕込めない)。ファイル API は symlink を作る手段を提供
//! しない。よって「検証 → path で操作」の TOCTOU は受容する(別プロセスが瞬間的に
//! symlink を差し込む経路が無い)。openat2 の NO_SYMLINKS は多層防御。

use crate::error::{AppError, AppResult};
use std::path::{Path, PathBuf};

/// 相対パスを正規化して root 相対の綺麗な `PathBuf` にする(root は空 PathBuf)。
/// 拒否:`..` 成分 / NUL / 制御文字。畳む:先頭・連続スラッシュ / `.` / 空成分。
/// 先頭の `/` は「假根のルート」を意味するものとして剥がす(宿主機の絶対パスではない)。
pub fn normalize_rel(rel: &str) -> AppResult<PathBuf> {
    if rel.as_bytes().contains(&0) {
        return Err(AppError::BadRequest(
            "パスに NUL を含めることはできません".into(),
        ));
    }
    let mut out = PathBuf::new();
    for comp in rel.split('/') {
        match comp {
            "" | "." => continue,
            ".." => {
                return Err(AppError::BadRequest(
                    "パスに '..' を含めることはできません".into(),
                ));
            }
            name => {
                if name.chars().any(char::is_control) {
                    return Err(AppError::BadRequest(
                        "パスに制御文字を含めることはできません".into(),
                    ));
                }
                out.push(name);
            }
        }
    }
    Ok(out)
}

/// 既存のファイル/ディレクトリを安全に解決して絶対パスを返す
/// (読み取り / 列挙 / 削除 / ダウンロード / 移動元 用)。存在しなければ 404。
pub fn resolve_existing(root: &Path, rel: &str) -> AppResult<PathBuf> {
    let clean = normalize_rel(rel)?;
    verify_existing(root, &clean)?;
    Ok(root.join(&clean))
}

/// `mkdir -p` を安全に行い(各段を syscall ゲートで検証)、作成したディレクトリの
/// 絶対パスを返す。mkdir エンドポイント本体 + アップロード/移動先の親作成に使う。
pub fn ensure_dir(root: &Path, rel: &str) -> AppResult<PathBuf> {
    let clean = normalize_rel(rel)?;
    ensure_dir_inner(root, &clean)?;
    Ok(root.join(&clean))
}

/// 書き込み対象(ファイル)を安全に解決して絶対パスを返す(アップロード / 移動先 用)。
/// 親ディレクトリ階層を安全に作成し、root 内であることを保証する。対象自体は未存在で良い。
/// root 自身(空パス)はファイルとして書けないので拒否。
pub fn resolve_for_write(root: &Path, rel: &str) -> AppResult<PathBuf> {
    let clean = normalize_rel(rel)?;
    if clean.as_os_str().is_empty() {
        return Err(AppError::BadRequest(
            "ルートそのものには書き込めません".into(),
        ));
    }
    match clean.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => ensure_dir_inner(root, parent)?,
        // 親は root。root が在ることだけ確認する(無ければ内部不整合)。
        _ => verify_root(root)?,
    }
    Ok(root.join(&clean))
}

// ===========================================================================
// Linux:openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)を唯一のゲートにする。
// ===========================================================================

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
    use rustix::io::Errno;
    use std::os::fd::OwnedFd;

    const RESOLVE: ResolveFlags = ResolveFlags::BENEATH.union(ResolveFlags::NO_SYMLINKS);

    /// root ディレクトリの fd を開く(以後の openat2 の基点)。
    fn open_root(root: &Path) -> AppResult<OwnedFd> {
        rustix::fs::open(
            root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| match e {
            Errno::NOENT | Errno::NOTDIR => AppError::NotFound,
            other => map_errno(other),
        })
    }

    /// openat2 の Errno を API エラーへ。ENOSYS は **fail-closed**(本番 Linux では
    /// 起こらない想定だが、起きたら安全側に倒して 500)。
    fn map_errno(e: Errno) -> AppError {
        match e {
            Errno::NOENT => AppError::NotFound,
            Errno::NOTDIR => AppError::BadRequest("途中がディレクトリではありません".into()),
            // BENEATH 違反(.. や絶対 symlink で root の外へ)/ NO_SYMLINKS 違反。
            Errno::XDEV | Errno::LOOP => AppError::Forbidden,
            Errno::NOSYS => AppError::Other(anyhow::anyhow!(
                "openat2 が利用できないカーネルです(ファイル操作を拒否しました)"
            )),
            other => AppError::Io(std::io::Error::from_raw_os_error(other.raw_os_error())),
        }
    }

    /// 空(root 自身)なら "." に。openat2 に空パスは渡せない。
    fn at(clean: &Path) -> &Path {
        if clean.as_os_str().is_empty() {
            Path::new(".")
        } else {
            clean
        }
    }

    pub(super) fn verify_root(root: &Path) -> AppResult<()> {
        open_root(root).map(|_| ())
    }

    /// 既存対象(ファイル/ディレクトリどちらでも)が root 内・symlink 無しであることを断言。
    pub(super) fn verify_existing(root: &Path, clean: &Path) -> AppResult<()> {
        let root_fd = open_root(root)?;
        // O_PATH:読み取り権が無くても「解決できるか」だけ確かめられる。
        openat2(
            &root_fd,
            at(clean),
            OFlags::PATH | OFlags::CLOEXEC,
            Mode::empty(),
            RESOLVE,
        )
        .map(|_| ())
        .map_err(map_errno)
    }

    /// `mkdir -p` を 1 段ずつ:各累積パスを openat2 で検証し、欠けていれば作る。
    /// 欠けた段の prefix は直前の反復(または root_fd)で検証済みなので、
    /// `create_dir`(lexically root 内・`..` 無し)は安全。
    pub(super) fn ensure_dir_inner(root: &Path, clean: &Path) -> AppResult<()> {
        let root_fd = open_root(root)?;
        let mut built = PathBuf::new();
        for comp in clean.components() {
            // normalize_rel 済みなので comp は通常成分のみ(Prefix/RootDir/.. は来ない)。
            built.push(comp);
            match openat2(
                &root_fd,
                &built,
                OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                RESOLVE,
            ) {
                Ok(_) => {}
                Err(Errno::NOENT) => {
                    if let Err(e) = std::fs::create_dir(root.join(&built)) {
                        // 競合(同時作成)で既に在るなら成功扱い。
                        if e.kind() != std::io::ErrorKind::AlreadyExists {
                            return Err(e.into());
                        }
                    }
                }
                Err(other) => return Err(map_errno(other)),
            }
        }
        Ok(())
    }
}

// ===========================================================================
// macOS 等(dev のみ):canonicalize + starts_with。openat2 が無いので軟い網。
// ===========================================================================

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::*;

    fn canon_root(root: &Path) -> AppResult<PathBuf> {
        root.canonicalize().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => AppError::NotFound,
            _ => AppError::Io(e),
        })
    }

    pub(super) fn verify_root(root: &Path) -> AppResult<()> {
        canon_root(root).map(|_| ())
    }

    pub(super) fn verify_existing(root: &Path, clean: &Path) -> AppResult<()> {
        let base = canon_root(root)?;
        let target = root.join(clean);
        let canon = target.canonicalize().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => AppError::NotFound,
            _ => AppError::Io(e),
        })?;
        if canon.starts_with(&base) {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }

    pub(super) fn ensure_dir_inner(root: &Path, clean: &Path) -> AppResult<()> {
        let base = canon_root(root)?;
        std::fs::create_dir_all(root.join(clean))?;
        let canon = root.join(clean).canonicalize()?;
        if canon.starts_with(&base) {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }
}

use imp::{ensure_dir_inner, verify_existing, verify_root};

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root() -> PathBuf {
        let p = std::env::temp_dir().join(format!("tsubomi-vol-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn normalize_collapses_and_strips() {
        assert_eq!(normalize_rel("a/b/c").unwrap(), PathBuf::from("a/b/c"));
        assert_eq!(normalize_rel("/a/b").unwrap(), PathBuf::from("a/b")); // 先頭スラッシュを剥ぐ
        assert_eq!(normalize_rel("a//b").unwrap(), PathBuf::from("a/b"));
        assert_eq!(normalize_rel("./a/./b").unwrap(), PathBuf::from("a/b"));
        assert_eq!(normalize_rel("").unwrap(), PathBuf::new()); // root
        assert_eq!(normalize_rel("/").unwrap(), PathBuf::new()); // root
    }

    #[test]
    fn normalize_rejects_traversal() {
        assert!(normalize_rel("..").is_err());
        assert!(normalize_rel("a/../b").is_err());
        assert!(normalize_rel("a/b/..").is_err());
        assert!(normalize_rel("../../etc/passwd").is_err());
        assert!(normalize_rel("/etc/../..").is_err());
    }

    #[test]
    fn normalize_rejects_nul_and_control() {
        assert!(normalize_rel("a\0b").is_err());
        assert!(normalize_rel("a\nb").is_err());
        assert!(normalize_rel("a\tb").is_err());
    }

    #[test]
    fn resolve_existing_ok_within_root() {
        let root = tmp_root();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/f.txt"), b"hi").unwrap();

        let p = resolve_existing(&root, "sub/f.txt").unwrap();
        assert_eq!(p, root.join("sub/f.txt"));
        // root 自身の解決も通る。
        assert_eq!(resolve_existing(&root, "").unwrap(), root.join(""));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_existing_missing_is_not_found() {
        let root = tmp_root();
        assert!(matches!(
            resolve_existing(&root, "nope.txt"),
            Err(AppError::NotFound)
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_existing_rejects_dotdot() {
        let root = tmp_root();
        assert!(resolve_existing(&root, "../escape").is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    /// 假根の中に外を指す symlink を張り、それ経由のアクセスが拒否されることを確認。
    /// Linux は openat2 NO_SYMLINKS(ELOOP)、macOS は canonicalize の脱出検出。
    #[cfg(unix)]
    #[test]
    fn resolve_existing_rejects_symlink_escape() {
        let root = tmp_root();
        let outside =
            std::env::temp_dir().join(format!("tsubomi-outside-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), b"top secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

        // link/secret.txt は物理的には outside/secret.txt を指すが、拒否されること。
        assert!(resolve_existing(&root, "link/secret.txt").is_err());

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn ensure_dir_creates_nested() {
        let root = tmp_root();
        let p = ensure_dir(&root, "a/b/c").unwrap();
        assert!(p.is_dir());
        assert_eq!(p, root.join("a/b/c"));
        // 冪等。
        assert!(ensure_dir(&root, "a/b/c").is_ok());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_for_write_prepares_parents() {
        let root = tmp_root();
        let p = resolve_for_write(&root, "docs/sub/readme.md").unwrap();
        assert_eq!(p, root.join("docs/sub/readme.md"));
        assert!(root.join("docs/sub").is_dir()); // 親まで作られている
        assert!(!p.exists()); // 対象自体はまだ作らない
        // root 自身は書き込み対象にできない。
        assert!(resolve_for_write(&root, "").is_err());
        assert!(resolve_for_write(&root, "../x").is_err());
        std::fs::remove_dir_all(&root).ok();
    }
}
