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
/// するため一意名)ので、後端 URL も deploy のたびに書き換わる。`ipallow` = 会社 IP 許可
/// リスト middleware(ipblock、`@file`)を挂けるか — visibility の company(true)/
/// public(false)。private はそもそもこの関数を呼ばない(ファイル自体を書かない)。
/// 値は全て平台生成なので YAML へそのまま埋めて安全。
pub fn write(
    state: &AppState,
    service_id: Uuid,
    subdomain: &str,
    container_name: &str,
    container_port: i32,
    ipallow: bool,
) -> AppResult<()> {
    let name = format!("svc-{service_id}");
    let host = format!("{}.{}", subdomain, state.config.domain);
    let backend = format!("http://{container_name}:{container_port}");
    let doc = build_service_doc(&name, &host, &backend, ipallow, state.config.tls);
    write_atomic(&route_path(state, service_id), &doc)
}

/// svc-<id>.yml の中身を組み立てる純粋関数(`write` の本体。`build_catchall_doc` と同型 =
/// テスト可能に分離)。`ipallow=false`(public)は middlewares 行を丸ごと出さない —
/// company と public の差はこの 1 行だけ(空許可リストは ipblock 側で fail-open なので、
/// 挂けない = 全網公開はこの行の有無で決まる)。
fn build_service_doc(name: &str, host: &str, backend: &str, ipallow: bool, tls: bool) -> String {
    let mut doc = String::new();
    doc.push_str("# 平台が自動生成(services/route.rs)。手で編集しない(deploy ごとに上書き)。\n");
    doc.push_str("http:\n");
    doc.push_str("  routers:\n");
    doc.push_str(&format!("    {name}:\n"));
    doc.push_str(&format!("      rule: \"Host(`{host}`)\"\n"));
    doc.push_str(&format!("      entryPoints: [\"{}\"]\n", entrypoint(tls)));
    doc.push_str(&format!("      service: \"{name}\"\n"));
    if ipallow {
        doc.push_str(&format!("      middlewares: [\"{}\"]\n", ipallow_ref()));
    }
    push_tls_block(&mut doc, tls);
    doc.push_str("  services:\n");
    doc.push_str(&format!("    {name}:\n"));
    doc.push_str("      loadBalancer:\n");
    doc.push_str("        servers:\n");
    doc.push_str(&format!("          - url: \"{backend}\"\n"));
    doc
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
    doc.push_str(&format!(
        "          - url: \"http://host.docker.internal:{port}\"\n"
    ));
    write_atomic(&state.config.traefik_dynamic_dir.join("apex.yml"), &doc)
}

/// 本番で「service の無い子域」を `/noservice` ページへ寄せる catch-all router を traefik に書く。
///
/// 仕組み:**最低優先度**(`priority: 1`)の `HostRegexp` で `<sub>.<domain>` 全体を受ける。
/// service の `Host(...)` router は優先度=ルール長で常にこれより上なので、**service があれば
/// 必ず service が勝ち全部 service に渡る**(service 自身の 404 もそのまま)。どの service router
/// にも当たらない子域(未デプロイ / 停止 / 削除済み = `remove` で `svc-<id>.yml` が消えた状態)
/// だけがここへ落ち、redirectRegex で `/noservice` へ **302**(後で同じ子域に service が来たら
/// 復活するので 301 にしない)。apex(`Host(<domain>)`)は正規表現が要求する「子域のドット」が
/// 無いので当たらない = リダイレクトループしない。registry 等の専用 router も優先度で上にいる。
///
/// dev(domain=localhost)は書かない(`*.localhost` 直アクセス。apex と同じ扱い)。起動時に 1 回。
///
/// TLS の扱い(両モード対応):CF tunnel(tls=false)= web entrypoint(HTTP)。直 VPS(tls=true)=
/// websecure + 空 `tls: {}`。**certResolver は付けない**:HostRegexp からは具体ドメインを導けず
/// LE は走らせられないし、ランダム子域の総当たりで LE レート制限を踏むのも防ぐ。直 VPS で死んだ
/// 子域も正しい証明書で出したいなら `*.<domain>` の DNS-01 ワイルドカード証明書を別途張る(無ければ
/// traefik 既定証明書 = ブラウザ警告。これは catch-all 以前から未ルート子域で同じ挙動)。
pub fn write_catchall(state: &AppState) -> AppResult<()> {
    let domain = &state.config.domain;
    if domain == "localhost" {
        return Ok(()); // dev は対象外
    }
    let doc = build_catchall_doc(domain, state.config.tls, state.config.bind_addr.port());
    write_atomic(&state.config.traefik_dynamic_dir.join("catchall.yml"), &doc)
}

/// catchall.yml の中身を組み立てる純粋関数(`write_catchall` の本体。テスト可能なように分離)。
fn build_catchall_doc(domain: &str, tls: bool, port: u16) -> String {
    // HostRegexp(Go 正規表現)用に domain のドットをエスケープ。`^.+\.<domain>$` =
    // 「1 ラベル以上 + ドット + ルートドメイン」⇒ 子域だけにマッチ(apex の裸ドメインは外れる)。
    // YAML 二重引用符の中なので backslash は `\\` で 1 個。最終的に traefik は `\.` を受け取る。
    let escaped = domain.replace('.', "\\\\.");

    let mut doc = String::new();
    doc.push_str("# 平台が自動生成(services/route.rs::write_catchall)。手で編集しない。\n");
    doc.push_str("http:\n");
    doc.push_str("  routers:\n");
    doc.push_str("    tsubomi-catchall:\n");
    doc.push_str(&format!("      rule: \"HostRegexp(`^.+\\\\.{escaped}$`)\"\n"));
    doc.push_str(&format!("      entryPoints: [\"{}\"]\n", entrypoint(tls)));
    // ★ 最低優先度。service の Host router(優先度=ルール長)に必ず負け、未ルート子域だけ拾う。
    doc.push_str("      priority: 1\n");
    doc.push_str("      service: \"tsubomi-catchall\"\n");
    // no-cache を **先(外側)** に置く:redirect が内側で生む 302 にも応答ヘッダが乗る
    // (middleware は先頭が最外殻 = 応答は最後に通る)。これが無いと、未デプロイ期に一度
    // 踏んだ 302 をブラウザがキャッシュし、後で service が来ても古い noservice に飛び続ける
    // (アドレス欄補完の `http://<sub>` 経由で特に起きやすい)。
    doc.push_str("      middlewares: [\"tsubomi-nocache@file\", \"tsubomi-noservice@file\"]\n");
    if tls {
        // certResolver なしで TLS router 化(既定 / ワイルドカード証明書で出す)。理由は doc 参照。
        doc.push_str("      tls: {}\n");
    }
    doc.push_str("  middlewares:\n");
    // 302 をブラウザにキャッシュさせない(no-store)。`permanent: false` だけでは不十分
    // — 仕様上 302 は非キャッシュでも、実ブラウザは scheme 込みのキーで残すことがある。
    doc.push_str("    tsubomi-nocache:\n");
    doc.push_str("      headers:\n");
    doc.push_str("        customResponseHeaders:\n");
    doc.push_str("          Cache-Control: \"no-store\"\n");
    doc.push_str("    tsubomi-noservice:\n");
    doc.push_str("      redirectRegex:\n");
    doc.push_str("        regex: \".*\"\n"); // URL 全体にマッチ → 固定先へ
    // 公開 scheme は常に https:呼び出し元 `write_catchall` が domain=localhost(唯一の http
    // = dev)を弾いて以降だけここへ来るため(`Config::service_url` の scheme 規則と同じ前提)。
    doc.push_str(&format!(
        "        replacement: \"https://{domain}/noservice\"\n"
    ));
    doc.push_str("        permanent: false\n"); // 302(後で service が来たら復活)
    doc.push_str("  services:\n");
    doc.push_str("    tsubomi-catchall:\n");
    doc.push_str("      loadBalancer:\n");
    doc.push_str("        servers:\n");
    // redirect middleware で短絡するので実到達しない(router に service は必須なので形式上 server を指す)。
    doc.push_str(&format!(
        "          - url: \"http://host.docker.internal:{port}\"\n"
    ));
    doc
}

/// service の stop / 削除時にルートファイルを消す(無ければ無視)。
pub fn remove(state: &AppState, service_id: Uuid) -> AppResult<()> {
    match std::fs::remove_file(route_path(state, service_id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// ipallow middleware の `@file` 参照文字列。writer(`build_service_doc`)と reader
/// (`parse_ipallow`)がここを共有 = 書く側と読む側の契約を 1 箇所に固定する。
fn ipallow_ref() -> String {
    format!("{}@file", crate::ipblock::TRAEFIK_MIDDLEWARE)
}

/// `svc-<id>.yml` の現実状態を読む:`(backend 容器名, ipallow 有無)`。ファイル無し / 解析不可は None。
/// reconcile の drift 判定は組で使うので **1 回の読みで両方**返す(二重読みは同一ファイルの別版を
/// 見得る + 無駄 I/O)。ipallow の不一致検出は public↔company の切替書込が失敗した fail-open
/// ドリフトを塞ぐ(公開範囲設計 §0-F)。
pub(crate) fn current(state: &AppState, service_id: Uuid) -> Option<(String, bool)> {
    let content = std::fs::read_to_string(route_path(state, service_id)).ok()?;
    Some((parse_backend_container(&content)?, parse_ipallow(&content)))
}

/// route ファイルが存在するか(private の期望状態 =「不存在」の判定用。読まずに stat だけ)。
pub(crate) fn exists(state: &AppState, service_id: Uuid) -> bool {
    route_path(state, service_id).exists()
}

/// route 内容に ipallow middleware の `@file` 参照があるか(`build_service_doc` の middlewares 行の逆)。
fn parse_ipallow(content: &str) -> bool {
    content.contains(&ipallow_ref())
}

/// `- url: "http://<name>:<port>"` 行から `<name>` を取り出す純粋関数(`write` の loadBalancer
/// server URL の逆)。`write` の出力フォーマットと密結合なので、両者がズレたら下のテストが落ちる。
fn parse_backend_container(content: &str) -> Option<String> {
    for line in content.lines() {
        let Some(rest) = line.trim().strip_prefix("- url:") else {
            continue;
        };
        let url = rest.trim().trim_matches('"');
        let after = url.strip_prefix("http://").unwrap_or(url);
        let name = after.split(':').next().unwrap_or("");
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
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
    use super::{
        build_catchall_doc, build_service_doc, parse_backend_container, parse_ipallow,
        parse_route_filename,
    };
    use uuid::Uuid;

    #[test]
    fn service_doc_ipallow_is_the_only_visibility_difference() {
        // company(ipallow=true)= middleware 行あり / public(false)= middlewares 行が丸ごと無い。
        let company = build_service_doc("svc-x", "a.example.com", "http://c:8080", true, false);
        assert!(company.contains("middlewares: [\"tsubomi-ipallow@file\"]"));
        assert!(parse_ipallow(&company));
        let public = build_service_doc("svc-x", "a.example.com", "http://c:8080", false, false);
        assert!(!public.contains("middlewares:"));
        assert!(!parse_ipallow(&public));
        // 差分は middlewares 行 1 行だけ(entrypoint / rule / backend は不変)。
        let diff: Vec<&str> = company.lines().filter(|l| !public.contains(l)).collect();
        assert_eq!(diff, vec!["      middlewares: [\"tsubomi-ipallow@file\"]"]);
        // parse_backend_container との往復(write フォーマット密結合の回帰)。
        assert_eq!(parse_backend_container(&public).as_deref(), Some("c"));
        // ipallow.yml など無関係な内容は false。
        assert!(!parse_ipallow("http:\n  middlewares: {}\n"));
    }

    #[test]
    fn service_doc_tls_branch_is_orthogonal_to_ipallow() {
        // tls 分岐(websecure + certResolver)は ipallow と直交して効く。
        let tls = build_service_doc("svc-x", "a.example.com", "http://c:8080", false, true);
        assert!(tls.contains("entryPoints: [\"websecure\"]"));
        assert!(tls.contains("certResolver: le"));
        let http = build_service_doc("svc-x", "a.example.com", "http://c:8080", true, false);
        assert!(http.contains("entryPoints: [\"web\"]"));
        assert!(!http.contains("certResolver"));
    }

    #[test]
    fn catchall_never_shadows_services_and_excludes_apex() {
        let doc = build_catchall_doc("tsubomi-app.com", false, 9090);
        // ★ 最低優先度:service の Host router(優先度=ルール長)に必ず負ける = service があれば素通し。
        assert!(doc.contains("priority: 1"));
        // `^.+\.` プレフィクスが「子域のドット」を必須にする → apex(裸ドメイン)は当たらない。
        // YAML 二重引用符内なので backslash は 2 個(traefik が受け取るのは `\.`)。
        assert!(doc.contains("rule: \"HostRegexp(`^.+\\\\.tsubomi-app\\\\.com$`)\""));
        // 302(permanent: false)で apex の /noservice へ。
        assert!(doc.contains("replacement: \"https://tsubomi-app.com/noservice\""));
        assert!(doc.contains("permanent: false"));
        // 302 はキャッシュさせない(no-store)+ redirect の外側に置く(302 にヘッダが乗るように)。
        // 補完の `http://<sub>` で踏んだ古い 302 が残り続ける事故(復活後も noservice)を防ぐ。
        assert!(doc.contains("Cache-Control: \"no-store\""));
        assert!(doc.contains("middlewares: [\"tsubomi-nocache@file\", \"tsubomi-noservice@file\"]"));
    }

    #[test]
    fn catchall_tls_branch_has_no_cert_resolver() {
        // CF tunnel(tls=false)= web entrypoint・tls ブロック無し。
        let http = build_catchall_doc("tsubomi-app.com", false, 9090);
        assert!(http.contains("entryPoints: [\"web\"]"));
        assert!(!http.contains("tls:"));
        // 直 VPS(tls=true)= websecure + 空 tls(certResolver は付けない:LE 総当たり回避)。
        let tls = build_catchall_doc("tsubomi-app.com", true, 9090);
        assert!(tls.contains("entryPoints: [\"websecure\"]"));
        assert!(tls.contains("tls: {}"));
        assert!(!tls.contains("certResolver"));
    }

    #[test]
    fn extracts_backend_container_name() {
        // `write` が出力する形(loadBalancer の server URL 行)から容器名を取り出す。
        let doc = "http:\n  services:\n    svc-x:\n      loadBalancer:\n        servers:\n          - url: \"http://tsubomi-abc123-deadbeef:8080\"\n";
        assert_eq!(
            parse_backend_container(doc).as_deref(),
            Some("tsubomi-abc123-deadbeef")
        );
        // url 行が無い(ipallow.yml 等)→ None。
        assert_eq!(parse_backend_container("http:\n  middlewares: {}\n"), None);
        assert_eq!(parse_backend_container(""), None);
    }

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
