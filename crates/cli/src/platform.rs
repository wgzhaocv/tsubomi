//! プラットフォーム / マシンの CPU アーキを表示するための定数とヘルパ。
//!
//! `host_arch()` = デプロイ対象プラットフォーム(tsubomi が動くホスト)のアーキ。**リリース時に確定する**:
//! `scripts/release-cli.sh` がデプロイ先ホストの実アーキを検出して `TSUBOMI_HOST_ARCH` に焼き込む
//! (どのマシンにデプロイしてもよい — arm64 を仮定しない)。`tbm --help` はオフライン(config ロード前)に
//! 生成されるため、サーバや config からは取れない — コンパイル時に焼いた値だけが help に載せられる。
//! 未設定(dev の `cargo run` 等)はビルド機のアーキにフォールバックする(dev ではビルド機 = ホスト)。
//!
//! `machine_arch()` = この tbm が動いているマシン(= `tbm deploy --local` のビルド機)のアーキ。
//! 両者を `tbm --help` / `tbm whoami` に出し、skill 冒頭にも `host_arch()` を埋め込む(`crate::skill`)。

/// デプロイ対象プラットフォームのアーキ(正規化済み: arm64 / amd64 等)。
/// リリース時に `TSUBOMI_HOST_ARCH` で焼き込み、未設定時はビルド機のアーキにフォールバック。
pub fn host_arch() -> &'static str {
    norm(match option_env!("TSUBOMI_HOST_ARCH") {
        Some(a) => a,
        None => std::env::consts::ARCH,
    })
}

/// この tbm が動いているマシン(= `tbm deploy --local` のビルド機)のアーキ(正規化済み)。
pub fn machine_arch() -> &'static str {
    norm(std::env::consts::ARCH)
}

/// docker `--platform linux/<arch>` と同じ語彙へ正規化(aarch64→arm64 / x86_64→amd64)。
/// 既知外の値はそのまま返す(誤って丸めない)。入力は `&'static`(`option_env!` の結果か
/// `std::env::consts::ARCH`)なので返り値も `&'static`。
fn norm(arch: &'static str) -> &'static str {
    match arch {
        "aarch64" | "arm64" => "arm64",
        "x86_64" | "amd64" => "amd64",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::norm;

    #[test]
    fn norm_maps_known_arches() {
        assert_eq!(norm("aarch64"), "arm64");
        assert_eq!(norm("arm64"), "arm64");
        assert_eq!(norm("x86_64"), "amd64");
        assert_eq!(norm("amd64"), "amd64");
        // 未知のアーキはそのまま返す(丸めない)。
        assert_eq!(norm("riscv64"), "riscv64");
    }
}
