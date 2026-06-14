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

/// traefik の entrypoint / certResolver 名。**compose.prod.yml の traefik command で定義する名前と
/// 一致させること**(static = compose が定義、dynamic = 平台が参照、の契約点)。ここを変えたら compose も。
pub(crate) const ENTRYPOINT_HTTP: &str = "web";
pub(crate) const ENTRYPOINT_TLS: &str = "websecure";
pub(crate) const CERT_RESOLVER: &str = "le";

/// router の entrypoint 名:tls(traefik 終端)= websecure、上流終端 = web。
pub(crate) fn entrypoint(tls: bool) -> &'static str {
    if tls { ENTRYPOINT_TLS } else { ENTRYPOINT_HTTP }
}

/// tls=true の router マッピングに tls/certResolver(LE)ブロックを足す(svc / apex / registry 共用)。
/// YAML はキー順不問なので呼び出し位置は問わない。
pub(crate) fn push_tls_block(doc: &mut String, tls: bool) {
    if tls {
        doc.push_str("      tls:\n");
        doc.push_str(&format!("        certResolver: {CERT_RESOLVER}\n"));
    }
}

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
    let tls = state.config.tls;

    let mut doc = String::new();
    doc.push_str("# 平台が自動生成(services/route.rs)。手で編集しない(deploy ごとに上書き)。\n");
    doc.push_str("http:\n");
    doc.push_str("  routers:\n");
    doc.push_str(&format!("    {name}:\n"));
    doc.push_str(&format!("      rule: \"Host(`{host}`)\"\n"));
    doc.push_str(&format!("      entryPoints: [\"{}\"]\n", entrypoint(tls)));
    doc.push_str(&format!("      service: \"{name}\"\n"));
    doc.push_str(&format!("      middlewares: [\"{mw}@file\"]\n"));
    push_tls_block(&mut doc, tls);
    doc.push_str("  services:\n");
    doc.push_str(&format!("    {name}:\n"));
    doc.push_str("      loadBalancer:\n");
    doc.push_str("        servers:\n");
    doc.push_str(&format!("          - url: \"{backend}\"\n"));

    write_atomic(&route_path(state, service_id), &doc)
}

/// 動的設定ファイルを原子的に置換する(tmp + rename。traefik が半端な内容を読まないように)。
/// route / registry が共有(`<name>.yml.tmp` は .yml で終わらないので traefik の glob 対象外)。
pub(crate) fn write_atomic(path: &std::path::Path, content: &str) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("yml.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 本番(tls)で apex(`<domain>` → 平台 server)を traefik に出す。server は host ネットなので
/// host-gateway 経由で届く(compose の traefik に `extra_hosts: host.docker.internal:host-gateway`)。
/// IP 許可リスト middleware は付けない(ログイン / owner 操作が届く必要があるため。registry と同じ免除)。
/// dev(tls=false)は何もしない(apex は vite / 直アクセス)。起動時に 1 回呼ぶ。
pub fn write_apex(state: &AppState) -> AppResult<()> {
    if !state.config.tls {
        return Ok(());
    }
    let domain = &state.config.domain;
    let port = state.config.bind_addr.port();
    let mut doc = String::new();
    doc.push_str("# 平台が自動生成(services/route.rs::write_apex)。手で編集しない。\n");
    doc.push_str("http:\n");
    doc.push_str("  routers:\n");
    doc.push_str("    tsubomi-apex:\n");
    doc.push_str(&format!("      rule: \"Host(`{domain}`)\"\n"));
    doc.push_str(&format!("      entryPoints: [\"{}\"]\n", entrypoint(true)));
    doc.push_str("      service: \"tsubomi-apex\"\n");
    push_tls_block(&mut doc, true); // apex は tls=true 時のみ書かれる(直 VPS)
    doc.push_str("  services:\n");
    doc.push_str("    tsubomi-apex:\n");
    doc.push_str("      loadBalancer:\n");
    doc.push_str("        servers:\n");
    doc.push_str(&format!("          - url: \"http://host.docker.internal:{port}\"\n"));
    write_atomic(&state.config.traefik_dynamic_dir.join("apex.yml"), &doc)
}

/// service の stop / 削除時にルートファイルを消す(無ければ無視)。
pub fn remove(state: &AppState, service_id: Uuid) -> AppResult<()> {
    match std::fs::remove_file(route_path(state, service_id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// dynamic dir 内の `svc-<uuid>.yml` ファイルから service_id を列挙する(reconcile の
/// 孤児 route 掃除用)。best-effort:dir が読めなければ空、命名規則に合わないファイルは無視。
pub(crate) fn list_service_ids(state: &AppState) -> Vec<Uuid> {
    let Ok(entries) = std::fs::read_dir(&state.config.traefik_dynamic_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| parse_route_filename(e.file_name().to_str()?))
        .collect()
}

/// `svc-<uuid>.yml` から uuid を取り出す純粋関数(write の `svc-{id}.yml` の逆)。
/// 平台が書く route ファイルだけを拾い、ipallow.yml や .tmp などは弾く。
fn parse_route_filename(name: &str) -> Option<Uuid> {
    let stem = name.strip_prefix("svc-")?.strip_suffix(".yml")?;
    Uuid::parse_str(stem).ok()
}

#[cfg(test)]
mod tests {
    use super::parse_route_filename;
    use uuid::Uuid;

    #[test]
    fn parses_only_service_route_files() {
        let id = Uuid::new_v4();
        assert_eq!(parse_route_filename(&format!("svc-{id}.yml")), Some(id));
        // 命名規則に合わないものは弾く(中間生成物 / 他用途 / 不正 uuid)。
        assert_eq!(parse_route_filename("ipallow.yml"), None);
        assert_eq!(parse_route_filename(&format!("svc-{id}.yml.tmp")), None);
        assert_eq!(parse_route_filename("svc-not-a-uuid.yml"), None);
        assert_eq!(parse_route_filename(&format!("{id}.yml")), None);
    }
}
