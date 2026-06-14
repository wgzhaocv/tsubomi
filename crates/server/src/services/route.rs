//! traefik の file provider 用に、service 1 つぶんの router + service 定義を書き出す。
//!
//! docker provider は使わない:Docker Engine 29 は最小 API を 1.40 に上げ、traefik の
//! docker クライアントは 1.24 に落ちて弾かれる(provider が全コンテナを見失い 404)。file
//! provider は docker API を一切触らないのでこれを回避する。後端へは安定コンテナ名
//! `tsubomi-<id>` を edge 網の docker DNS で解決して到達する(名前解決は provider とは別)。
//!
//! 役割分担:ipblock が会社 IP 許可リストの middleware(ipallow.yml)を書き、ここは各
//! service の router(その middleware を `@file` 参照)+ service を `svc-<id>.yml` に書く。
//! traefik は同じディレクトリの両ファイルを併せて読む。
//!
//! ★ 形式は **YAML**(ipblock と同じ)。traefik の directory file provider は実測で .yml は
//!   読むが .json を読み込まない(ディレクトリ監視に追加はされるが設定にマージされない)ため。

use crate::error::AppResult;
use crate::state::AppState;
use std::path::PathBuf;
use uuid::Uuid;

/// service の動的設定ファイルのパス(`<dir>/svc-<id>.yml`)。
fn route_path(state: &AppState, service_id: Uuid) -> PathBuf {
    state
        .config
        .traefik_dynamic_dir
        .join(format!("svc-{service_id}.yml"))
}

/// router + service を 1 ファイル原子的に書く(traefik が watch してホットリロード)。
/// router/service 名 = `svc-<id>`(安定、ファイルは service ごと 1 枚)、**後端 = 渡された
/// コンテナ名**。start-first swap では deploy ごとにコンテナ名が変わる(新旧が一瞬共存
/// するため一意名)ので、後端 URL も deploy のたびに書き換わる。middleware は会社 IP 許可
/// リスト(ipblock、`@file`)。値は全て平台生成なので YAML へそのまま埋めて安全。
pub fn write(
    state: &AppState,
    service_id: Uuid,
    subdomain: &str,
    container_name: &str,
    container_port: i32,
) -> AppResult<()> {
    let name = format!("svc-{service_id}");
    let host = format!("{}.{}", subdomain, state.config.domain);
    let backend = format!("http://{container_name}:{container_port}");
    let mw = crate::ipblock::TRAEFIK_MIDDLEWARE;

    let mut doc = String::new();
    doc.push_str("# 平台が自動生成(services/route.rs)。手で編集しない(deploy ごとに上書き)。\n");
    doc.push_str("http:\n");
    doc.push_str("  routers:\n");
    doc.push_str(&format!("    {name}:\n"));
    doc.push_str(&format!("      rule: \"Host(`{host}`)\"\n"));
    doc.push_str("      entryPoints: [\"web\"]\n");
    doc.push_str(&format!("      service: \"{name}\"\n"));
    doc.push_str(&format!("      middlewares: [\"{mw}@file\"]\n"));
    doc.push_str("  services:\n");
    doc.push_str(&format!("    {name}:\n"));
    doc.push_str("      loadBalancer:\n");
    doc.push_str("        servers:\n");
    doc.push_str(&format!("          - url: \"{backend}\"\n"));

    let path = route_path(state, service_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // tmp + rename:traefik が半端な内容を読まないように原子的に置換する。
    let tmp = path.with_extension("yml.tmp");
    std::fs::write(&tmp, doc)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// service 削除時にルートファイルを消す(無ければ無視)。lifecycle(S7)で使う。
#[allow(dead_code)]
pub fn remove(state: &AppState, service_id: Uuid) -> AppResult<()> {
    match std::fs::remove_file(route_path(state, service_id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
