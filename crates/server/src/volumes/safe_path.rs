//! 假根(volume の host_path)内にユーザ/AI 由来の相対パスを必ず収める —
//! **唯一のハード安全境界**(design v2 §6 / tech-design §7)。漏らせば假根が
//! 穿透され、他人や宿主機のファイルに届く。
//!
//! 二段構え:
//!  1. `normalize_rel` — 純粋・プラットフォーム非依存。`..` / NUL / 制御文字を拒否し、
//!     先頭スラッシュ(「/ から始まる」假根の見た目)を剥がして root 相対に畳む。
//!     これだけで *文字列*トラバーサル(`../../etc/passwd`、絶対パス)を落とす。
//!  2. syscall ゲート — 既存ディレクトリ経由のシンボリックリンク越えを塞ぐ。
//!     - **Linux(本番)**:`openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)` で root 配下の
//!       fd を取り、**実際の操作(read/列挙/削除/改名/作成)は全部その fd 相対**で行う
//!       (`openat`/`mkdirat`/`unlinkat`/`renameat`)。「検証 → 裸 path で再操作」の TOCTOU を
//!       構造的に排す:検証した瞬間の inode を fd でピン留めし、経路成分のすり替えを無効化する。
//!     - **macOS 等(dev のみ)**:`canonicalize` で実体解決し `starts_with(root)` を断言する
//!       path ベースの軟い網。openat2 が無いので fd 相対化はしない。**本番は必ず Linux**。
//!
//! 脅威モデル(改訂):**volume は service 注入で rw bind マウントされコンテナの中から書ける**
//! (inject.rs)。つまりテナントの容器が自分の volume 内に symlink を仕込める = サーバは
//! 「唯一の書き手」ではない。だから「検証してから path で操作」は危険(検証と操作の隙に
//! symlink へ差し替えられ、root のサーバが宿主機 / 他テナントの path を辿らされる)。これを
//! 上記の fd 相対操作で塞ぐ。openat2 の NO_SYMLINKS は多層防御の一枚。

use crate::error::{AppError, AppResult};
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use tsubomi_shared::FileEntryDto;
use uuid::Uuid;

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

// ===========================================================================
// 公開 API(全て normalize_rel を通してから imp の fd 相対操作へ委譲)。
// volumes.rs のファイル API は **裸 PathBuf を一切受け取らない** — 安全境界を跨ぐ
// 操作はこのモジュール内(fd 相対)で完結する。
// ===========================================================================

/// ディレクトリを列挙する(list_files)。`(正規化済み相対パス文字列, エントリ列)` を返す。
/// 列挙は root 配下に開いた dir fd から行い、各エントリの種別/サイズ/更新時刻は
/// `AT_SYMLINK_NOFOLLOW` の statat で取る(symlink を辿らない)。`max` 超過は明示エラー。
pub fn read_dir(root: &Path, rel: &str, max: usize) -> AppResult<(String, Vec<FileEntryDto>)> {
    let clean = normalize_rel(rel)?;
    let entries = imp::read_dir(root, &clean, max)?;
    Ok((clean.to_string_lossy().into_owned(), entries))
}

/// 既存ファイルを読み取り用に開く(download)。`(File, サイズ, ファイル名)` を返す。
/// dir なら BadRequest、不在なら NotFound。open 自体が検証(openat2 NO_SYMLINKS)= TOCTOU 無し。
pub fn open_for_read(root: &Path, rel: &str) -> AppResult<(std::fs::File, u64, String)> {
    let clean = normalize_rel(rel)?;
    let (file, len) = imp::open_for_read(root, &clean)?;
    let filename = clean
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".into());
    Ok((file, len, filename))
}

/// `mkdir -p`(各段を fd 相対 mkdirat で作る)。
pub fn ensure_dir(root: &Path, rel: &str) -> AppResult<()> {
    let clean = normalize_rel(rel)?;
    imp::ensure_dir(root, &clean)
}

/// ファイル / ディレクトリ(再帰)を削除する(delete_entry)。root 自身は拒否。
pub fn remove(root: &Path, rel: &str) -> AppResult<()> {
    let clean = normalize_rel(rel)?;
    if clean.as_os_str().is_empty() {
        return Err(AppError::BadRequest(
            "ルートは削除できません(ボリュームごと削除してください)".into(),
        ));
    }
    imp::remove(root, &clean)
}

/// 同一 volume 内の rename / move(move_entry)。移動先が既にあれば Conflict
/// (黙って上書きしない — 事故・データ損失を防ぐ)。どちらも root 自身は拒否。
pub fn rename(root: &Path, from: &str, to: &str) -> AppResult<()> {
    let from_c = normalize_rel(from)?;
    let to_c = normalize_rel(to)?;
    if from_c.as_os_str().is_empty() || to_c.as_os_str().is_empty() {
        return Err(AppError::BadRequest(
            "ルートそのものは移動できません".into(),
        ));
    }
    imp::rename(root, &from_c, &to_c)
}

/// アップロードの書き込みを開始する(upload)。親を `mkdir -p` し、同一ディレクトリの
/// ユニークな一時ファイルを `O_CREAT|O_EXCL|O_NOFOLLOW` で開いて返す。呼び出し側は
/// `File` にストリーム書き込み後、成功なら `commit`(atomic rename で dest を置換)、
/// 失敗なら `abort`(tmp を消す)を呼ぶ。tmp + atomic rename なので途中失敗で既存 dest を壊さない。
pub fn begin_write(root: &Path, rel: &str) -> AppResult<(std::fs::File, UploadCommit)> {
    let clean = normalize_rel(rel)?;
    if clean.as_os_str().is_empty() {
        return Err(AppError::BadRequest(
            "ルートそのものには書き込めません".into(),
        ));
    }
    imp::begin_write(root, &clean)
}

pub use imp::UploadCommit;

// ===========================================================================
// Linux:openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)で root 配下の fd を取り、
// 実操作はその fd 相対(openat / mkdirat / unlinkat / renameat)で完結させる。
// ===========================================================================

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use rustix::fs::{
        AtFlags, Dir, FileType, Mode, OFlags, RenameFlags, ResolveFlags, mkdirat, openat, openat2,
        renameat_with, statat, unlinkat,
    };
    use rustix::io::Errno;
    use std::ffi::OsStr;
    use std::os::fd::{AsRawFd, OwnedFd};

    const RESOLVE: ResolveFlags = ResolveFlags::BENEATH.union(ResolveFlags::NO_SYMLINKS);
    /// 新規ディレクトリ / ファイルの作成モード(所有者のみ。実体は単一 uid のサーバ所有)。
    const DIR_MODE: Mode = Mode::RWXU; // 0o700
    const FILE_MODE: Mode = Mode::RUSR.union(Mode::WUSR); // 0o600

    /// 書き込み完了後の収尾(atomic rename / 失敗時の tmp 掃除)。親ディレクトリ fd を
    /// 抱えたまま renameat / unlinkat するので、検証済みの親 inode に対して原子的に確定する。
    pub struct UploadCommit {
        parent: OwnedFd,
        tmp: std::ffi::OsString,
        dest: std::ffi::OsString,
    }
    impl UploadCommit {
        pub fn commit(self) -> AppResult<()> {
            // 上書き許可(既存 dest の置換 = アップロードの上書き意味論)。同一親 fd 内の rename。
            rustix::fs::renameat(&self.parent, &*self.tmp, &self.parent, &*self.dest)
                .map_err(map_errno)
        }
        pub fn abort(self) {
            let _ = unlinkat(&self.parent, &*self.tmp, AtFlags::empty());
        }
    }

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
            Errno::EXIST => AppError::Conflict("移動先が既に存在します".into()),
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

    /// `clean` を (親の相対パス, 末尾の名前) に割る。空(root)は不正(呼び出し側で弾く想定だが念のため)。
    fn split_final(clean: &Path) -> AppResult<(&Path, &OsStr)> {
        let name = clean
            .file_name()
            .ok_or_else(|| AppError::BadRequest("ルートそのものは対象にできません".into()))?;
        let parent = clean.parent().unwrap_or_else(|| Path::new(""));
        Ok((parent, name))
    }

    /// root 配下の(既存)ディレクトリを読み取り用に開く(NO_SYMLINKS)。dir でなければ ENOTDIR。
    fn open_dir(root_fd: &OwnedFd, rel: &Path) -> AppResult<OwnedFd> {
        openat2(
            root_fd,
            at(rel),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            RESOLVE,
        )
        .map_err(map_errno)
    }

    pub(super) fn read_dir(root: &Path, clean: &Path, max: usize) -> AppResult<Vec<FileEntryDto>> {
        let root_fd = open_root(root)?;
        let dir_fd = open_dir(&root_fd, clean)?;
        let mut entries: Vec<FileEntryDto> = Vec::new();
        // Dir は fd を dup して読むので、同じ dir_fd を statat に使い続けられる。
        let dir = Dir::read_from(&dir_fd).map_err(map_errno)?;
        for item in dir {
            let entry = item.map_err(map_errno)?;
            let name = entry.file_name();
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            if entries.len() >= max {
                return Err(AppError::BadRequest(format!(
                    "ディレクトリのエントリが多すぎます(上限 {max})。サブディレクトリで絞ってください"
                )));
            }
            // 種別 / サイズ / 更新時刻は symlink を辿らず取る(エントリ自身の stat)。
            let st = statat(&dir_fd, name, AtFlags::SYMLINK_NOFOLLOW).map_err(map_errno)?;
            let is_dir = FileType::from_raw_mode(st.st_mode) == FileType::Directory;
            entries.push(FileEntryDto {
                name: String::from_utf8_lossy(name.to_bytes()).into_owned(),
                is_dir,
                size: if is_dir { 0 } else { st.st_size as u64 },
                modified: DateTime::<Utc>::from_timestamp(st.st_mtime, st.st_mtime_nsec as u32),
            });
        }
        Ok(entries)
    }

    pub(super) fn open_for_read(root: &Path, clean: &Path) -> AppResult<(std::fs::File, u64)> {
        let root_fd = open_root(root)?;
        // open 自体が検証:NO_SYMLINKS なので途中/末尾の symlink は ELOOP で弾かれる。
        let fd = openat2(
            &root_fd,
            at(clean),
            OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
            RESOLVE,
        )
        .map_err(map_errno)?;
        let file = std::fs::File::from(fd);
        let meta = file.metadata()?;
        if meta.is_dir() {
            return Err(AppError::BadRequest(
                "ディレクトリはダウンロードできません".into(),
            ));
        }
        Ok((file, meta.len()))
    }

    /// `mkdir -p` を 1 段ずつ:累積した親 fd に対して openat2(NO_SYMLINKS)で子を引き、
    /// 欠けていれば **その親 fd 相対に** mkdirat で作る。各段が直前段の fd にピン留めされるので、
    /// 経路成分を symlink にすり替えられても辿らない。最終的な親 fd を返す。
    fn ensure_dir_fd(root: &Path, clean: &Path) -> AppResult<OwnedFd> {
        let mut dir_fd = open_root(root)?;
        for comp in clean.components() {
            let name = comp.as_os_str();
            match openat2(
                &dir_fd,
                name,
                OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                RESOLVE,
            ) {
                Ok(child) => dir_fd = child,
                Err(Errno::NOENT) => {
                    match mkdirat(&dir_fd, name, DIR_MODE) {
                        Ok(()) => {}
                        // 競合(同時作成)で既に在るなら成功扱い。
                        Err(Errno::EXIST) => {}
                        Err(e) => return Err(map_errno(e)),
                    }
                    dir_fd = openat2(
                        &dir_fd,
                        name,
                        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                        Mode::empty(),
                        RESOLVE,
                    )
                    .map_err(map_errno)?;
                }
                Err(other) => return Err(map_errno(other)),
            }
        }
        Ok(dir_fd)
    }

    pub(super) fn ensure_dir(root: &Path, clean: &Path) -> AppResult<()> {
        ensure_dir_fd(root, clean).map(|_| ())
    }

    pub(super) fn remove(root: &Path, clean: &Path) -> AppResult<()> {
        let (parent_rel, name) = split_final(clean)?;
        let root_fd = open_root(root)?;
        let parent_fd = open_dir(&root_fd, parent_rel)?;
        // 種別を symlink 非追従で判定。symlink は「非 dir」と出るので unlinkat で link 自体を外す。
        let st = statat(&parent_fd, name, AtFlags::SYMLINK_NOFOLLOW).map_err(map_errno)?;
        if FileType::from_raw_mode(st.st_mode) == FileType::Directory {
            // 再帰削除は std に委譲(1.58+ は symlink を辿らず O_NOFOLLOW で TOCTOU 耐性を持つ)。
            // 親 fd を /proc/self/fd で固定したパス経由で叩くので、親より上の経路すり替えは無効。
            // name が(statat 後に)symlink へ差し替えられても std は O_NOFOLLOW で開けず失敗する
            //(= 越境はせず、ただのエラーで止まる)。
            let target = proc_fd_path(&parent_fd, name);
            std::fs::remove_dir_all(target)?;
        } else {
            unlinkat(&parent_fd, name, AtFlags::empty()).map_err(map_errno)?;
        }
        Ok(())
    }

    pub(super) fn rename(root: &Path, from: &Path, to: &Path) -> AppResult<()> {
        let (from_parent_rel, from_name) = split_final(from)?;
        let (to_parent_rel, to_name) = split_final(to)?;
        let root_fd = open_root(root)?;
        // 移動先の親階層を用意してから両親 fd を開く。
        let to_parent_fd = ensure_dir_fd(root, to_parent_rel)?;
        let from_parent_fd = open_dir(&root_fd, from_parent_rel)?;
        // NOREPLACE:移動先が在れば EEXIST → Conflict(上書きしない)。from 不在は ENOENT → NotFound。
        match renameat_with(
            &from_parent_fd,
            from_name,
            &to_parent_fd,
            to_name,
            RenameFlags::NOREPLACE,
        ) {
            Ok(()) => Ok(()),
            Err(e) => Err(map_errno(e)),
        }
    }

    pub(super) fn begin_write(root: &Path, clean: &Path) -> AppResult<(std::fs::File, UploadCommit)> {
        let (parent_rel, name) = split_final(clean)?;
        // 親を用意してその fd を抱える(commit の rename / abort の unlink もこの fd 相対)。
        let parent = ensure_dir_fd(root, parent_rel)?;
        let tmp = std::ffi::OsString::from(format!(".tbm-upload-{}.tmp", Uuid::new_v4()));
        // O_EXCL|O_NOFOLLOW:既存(symlink 含む)を踏まないユニーク新規。
        let fd = openat(
            &parent,
            &tmp,
            OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::CLOEXEC,
            FILE_MODE,
        )
        .map_err(map_errno)?;
        Ok((
            std::fs::File::from(fd),
            UploadCommit {
                parent,
                tmp,
                dest: name.to_os_string(),
            },
        ))
    }

    /// 開いている fd を指す `/proc/self/fd/<n>/<name>`(親 inode を fd でピン留めしたパス)。
    fn proc_fd_path(fd: &OwnedFd, name: &OsStr) -> PathBuf {
        Path::new("/proc/self/fd")
            .join(fd.as_raw_fd().to_string())
            .join(name)
    }
}

// ===========================================================================
// macOS 等(dev のみ):canonicalize + starts_with の path ベース軟い網。
// openat2 が無いので fd 相対化はしない。本番は必ず Linux なのでハード保証は Linux 側が持つ。
// ===========================================================================

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::*;

    /// dev の収尾(path ベース)。
    pub struct UploadCommit {
        tmp: PathBuf,
        dest: PathBuf,
    }
    impl UploadCommit {
        pub fn commit(self) -> AppResult<()> {
            std::fs::rename(&self.tmp, &self.dest)?;
            Ok(())
        }
        pub fn abort(self) {
            let _ = std::fs::remove_file(&self.tmp);
        }
    }

    fn canon_root(root: &Path) -> AppResult<PathBuf> {
        root.canonicalize().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => AppError::NotFound,
            _ => AppError::Io(e),
        })
    }

    /// 既存対象の実体パスを返し、root 配下であることを確認(無ければ NotFound、脱出は Forbidden)。
    fn resolve_existing(root: &Path, clean: &Path) -> AppResult<PathBuf> {
        let base = canon_root(root)?;
        let canon = root.join(clean).canonicalize().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => AppError::NotFound,
            _ => AppError::Io(e),
        })?;
        if canon.starts_with(&base) {
            Ok(canon)
        } else {
            Err(AppError::Forbidden)
        }
    }

    /// 書き込み先:親を作成し、親の実体が root 配下か確認して dest パス(未存在で良い)を返す。
    fn resolve_for_write(root: &Path, clean: &Path) -> AppResult<PathBuf> {
        let base = canon_root(root)?;
        let name = clean
            .file_name()
            .ok_or_else(|| AppError::BadRequest("ルートそのものには書き込めません".into()))?;
        let parent_rel = clean.parent().unwrap_or_else(|| Path::new(""));
        if !parent_rel.as_os_str().is_empty() {
            ensure_dir(root, parent_rel)?;
        }
        let canon_parent = root
            .join(parent_rel)
            .canonicalize()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => AppError::NotFound,
                _ => AppError::Io(e),
            })?;
        if !canon_parent.starts_with(&base) {
            return Err(AppError::Forbidden);
        }
        Ok(canon_parent.join(name))
    }

    pub(super) fn read_dir(root: &Path, clean: &Path, max: usize) -> AppResult<Vec<FileEntryDto>> {
        let dir = resolve_existing(root, clean)?;
        if !std::fs::metadata(&dir)?.is_dir() {
            return Err(AppError::BadRequest(
                "指定パスはディレクトリではありません".into(),
            ));
        }
        let mut entries: Vec<FileEntryDto> = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            if entries.len() >= max {
                return Err(AppError::BadRequest(format!(
                    "ディレクトリのエントリが多すぎます(上限 {max})。サブディレクトリで絞ってください"
                )));
            }
            let entry = entry?;
            // file_type / metadata は symlink を辿らない(エントリ自身)。
            let ft = entry.file_type()?;
            let md = entry.metadata().ok();
            let is_dir = ft.is_dir();
            entries.push(FileEntryDto {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir,
                size: if is_dir {
                    0
                } else {
                    md.as_ref().map(|m| m.len()).unwrap_or(0)
                },
                modified: md
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .map(DateTime::<Utc>::from),
            });
        }
        Ok(entries)
    }

    pub(super) fn open_for_read(root: &Path, clean: &Path) -> AppResult<(std::fs::File, u64)> {
        let p = resolve_existing(root, clean)?;
        let meta = std::fs::metadata(&p)?;
        if meta.is_dir() {
            return Err(AppError::BadRequest(
                "ディレクトリはダウンロードできません".into(),
            ));
        }
        Ok((std::fs::File::open(&p)?, meta.len()))
    }

    pub(super) fn ensure_dir(root: &Path, clean: &Path) -> AppResult<()> {
        let base = canon_root(root)?;
        std::fs::create_dir_all(root.join(clean))?;
        let canon = root.join(clean).canonicalize()?;
        if canon.starts_with(&base) {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }

    pub(super) fn remove(root: &Path, clean: &Path) -> AppResult<()> {
        let p = resolve_existing(root, clean)?;
        if std::fs::metadata(&p)?.is_dir() {
            std::fs::remove_dir_all(&p)?;
        } else {
            std::fs::remove_file(&p)?;
        }
        Ok(())
    }

    pub(super) fn rename(root: &Path, from: &Path, to: &Path) -> AppResult<()> {
        let from_p = resolve_existing(root, from)?;
        let to_p = resolve_for_write(root, to)?;
        if std::fs::symlink_metadata(&to_p).is_ok() {
            return Err(AppError::Conflict("移動先が既に存在します".into()));
        }
        std::fs::rename(&from_p, &to_p)?;
        Ok(())
    }

    pub(super) fn begin_write(root: &Path, clean: &Path) -> AppResult<(std::fs::File, UploadCommit)> {
        let dest = resolve_for_write(root, clean)?;
        let parent = dest
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| root.to_path_buf());
        let tmp = parent.join(format!(".tbm-upload-{}.tmp", Uuid::new_v4()));
        let file = std::fs::File::create(&tmp)?;
        Ok((file, UploadCommit { tmp, dest }))
    }
}

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
    fn read_and_list_within_root() {
        let root = tmp_root();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/f.txt"), b"hi").unwrap();

        let (rel, entries) = read_dir(&root, "sub", 100).unwrap();
        assert_eq!(rel, "sub");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "f.txt");
        assert!(!entries[0].is_dir);
        assert_eq!(entries[0].size, 2);

        let (file, len, name) = open_for_read(&root, "sub/f.txt").unwrap();
        assert_eq!(len, 2);
        assert_eq!(name, "f.txt");
        drop(file);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn open_missing_is_not_found() {
        let root = tmp_root();
        assert!(matches!(
            open_for_read(&root, "nope.txt"),
            Err(AppError::NotFound)
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn open_rejects_dotdot() {
        let root = tmp_root();
        assert!(open_for_read(&root, "../escape").is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn download_rejects_dir() {
        let root = tmp_root();
        std::fs::create_dir_all(root.join("d")).unwrap();
        assert!(matches!(
            open_for_read(&root, "d"),
            Err(AppError::BadRequest(_))
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    /// 假根の中に外を指す symlink を張り、それ経由のアクセスが拒否されることを確認。
    /// Linux は openat2 NO_SYMLINKS(ELOOP→Forbidden)、macOS は canonicalize の脱出検出。
    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        let root = tmp_root();
        let outside =
            std::env::temp_dir().join(format!("tsubomi-outside-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), b"top secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

        // link/secret.txt は物理的には outside/secret.txt を指すが、拒否されること。
        assert!(open_for_read(&root, "link/secret.txt").is_err());
        assert!(read_dir(&root, "link", 100).is_err());

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn ensure_dir_creates_nested() {
        let root = tmp_root();
        ensure_dir(&root, "a/b/c").unwrap();
        assert!(root.join("a/b/c").is_dir());
        // 冪等。
        assert!(ensure_dir(&root, "a/b/c").is_ok());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn upload_writes_then_commits() {
        let root = tmp_root();
        let (file, commit) = begin_write(&root, "docs/sub/readme.md").unwrap();
        assert!(root.join("docs/sub").is_dir()); // 親まで作られている
        {
            use std::io::Write;
            let mut f = file;
            f.write_all(b"hello").unwrap();
            f.flush().unwrap();
        }
        commit.commit().unwrap();
        assert_eq!(std::fs::read(root.join("docs/sub/readme.md")).unwrap(), b"hello");
        // root 自身は書き込み対象にできない。
        assert!(begin_write(&root, "").is_err());
        assert!(begin_write(&root, "../x").is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn upload_abort_leaves_dest_untouched() {
        let root = tmp_root();
        std::fs::write(root.join("keep.txt"), b"original").unwrap();
        let (file, commit) = begin_write(&root, "keep.txt").unwrap();
        drop(file);
        commit.abort();
        assert_eq!(std::fs::read(root.join("keep.txt")).unwrap(), b"original");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn remove_file_and_dir() {
        let root = tmp_root();
        std::fs::write(root.join("f.txt"), b"x").unwrap();
        std::fs::create_dir_all(root.join("d/e")).unwrap();
        std::fs::write(root.join("d/e/g.txt"), b"y").unwrap();

        remove(&root, "f.txt").unwrap();
        assert!(!root.join("f.txt").exists());
        remove(&root, "d").unwrap();
        assert!(!root.join("d").exists());
        // root は消せない。
        assert!(remove(&root, "").is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rename_moves_and_rejects_existing() {
        let root = tmp_root();
        std::fs::write(root.join("a.txt"), b"x").unwrap();
        rename(&root, "a.txt", "sub/b.txt").unwrap();
        assert!(!root.join("a.txt").exists());
        assert_eq!(std::fs::read(root.join("sub/b.txt")).unwrap(), b"x");

        // 移動先が既にあれば Conflict。
        std::fs::write(root.join("c.txt"), b"z").unwrap();
        assert!(matches!(
            rename(&root, "c.txt", "sub/b.txt"),
            Err(AppError::Conflict(_))
        ));
        std::fs::remove_dir_all(&root).ok();
    }
}
