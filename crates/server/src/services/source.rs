//! 第 3 のデプロイ経路(deploy-source):**サーバ側で**既成イメージを pull、または
//! コンテキスト無し Dockerfile を build して内部 registry へ push し、既存パイプライン
//! (deploy::run_digest)で起こす。GitHub にもユーザ機の docker にも依存しない —
//! 「ビルド環境が無いと現成イメージすら部署できない」という §4 闸門の意味論的な穴を塞ぐ。
//!
//! 設計の要点:
//! - **配方は Dockerfile(世界共通の形式)か image 参照**。平台私有の DSL は作らない。
//!   配方原文は `service_details.source_kind / source_spec` に期望状態として保存する。
//! - **無 context の一線**:COPY / ADD は拒否。ファイルが要る = app のコード = 従来の
//!   GitHub / --local 経路の領分(この一線を越えると CI の再発明になる)。
//! - **取得は非同期**(202 即返し + tokio::spawn):外部 pull は分単位になり得て、
//!   CF tunnel 経由の長い HTTP は 100s で切られる。進捗・失敗は deploys 行に載る
//!   (placeholder digest 'pending' → 取得成功で実 digest に UPDATE)。
//! - 取得後は既存の run_digest に合流:deploy_lock / swap / readiness / rollback / GC の
//!   不変量はそのまま(digest は内部 registry の per-service repo に必ず実在する)。

use crate::auth::AuthCtx;
use crate::databases::audit;
use crate::error::{AppError, AppResult};
use crate::services::deploy::{self, DeployTrigger};
use crate::services::{docker, ensure_owned, registry};
use crate::state::AppState;
use anyhow::anyhow;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use futures_util::FutureExt;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::panic::AssertUnwindSafe;
use tsubomi_shared::{
    DeploySourceReq, DeploySourceResp, MAX_DOCKERFILE_BYTES, SOURCE_KIND_DOCKERFILE,
    SOURCE_KIND_IMAGE,
};
use uuid::Uuid;

const MAX_IMAGE_REF_LEN: usize = 512;
/// 取得完了までの deploys.image_digest プレースホルダ(NOT NULL のため)。digest 形でない
/// 値として rollback 守卫 / GC skip が識別する(is_sha256_digest = false)。
pub(crate) const PLACEHOLDER_DIGEST: &str = "pending";

/// コンテキスト無しビルドで許す Dockerfile 命令の白名単。
/// COPY / ADD(ファイル要求 = 無 context の契約違反)、VOLUME(匿名 volume はデータが
/// deploy ごとに消える罠 — 永続は平台の volume 注入で)、ONBUILD 等は拒否。
const ALLOWED_INSTRUCTIONS: &[&str] = &[
    "FROM",
    "RUN",
    "ENV",
    "ARG",
    "CMD",
    "ENTRYPOINT",
    "EXPOSE",
    "WORKDIR",
    "USER",
    "LABEL",
    "HEALTHCHECK",
    "SHELL",
    "STOPSIGNAL",
];

/// Dockerfile を検証する(白名単方式)。行継続 `\` を論理行に畳んでから先頭語を判定。
/// コメント / 空行 / parser directive(`# syntax=` 等)は許容。
fn validate_dockerfile(text: &str) -> AppResult<()> {
    if text.trim().is_empty() {
        return Err(AppError::BadRequest("Dockerfile が空です".into()));
    }
    if text.len() > MAX_DOCKERFILE_BYTES {
        return Err(AppError::BadRequest(format!(
            "Dockerfile が大きすぎます(上限 {}KiB)",
            MAX_DOCKERFILE_BYTES / 1024
        )));
    }
    // parser directive `# escape=` は継続文字を変える(既定 `\`)。非既定を許すと logical_lines の
    // `\` 畳みが Docker の実挙動とズレ、畳んだ結果に隠れた COPY 等をすり抜けさせ得る(codex 指摘)。
    // 先頭の空白/コメント行だけが directive になり得る = 素朴に全文から検出して拒否する。
    for line in text.lines() {
        let t = line.trim();
        let low = t.to_ascii_lowercase();
        if let Some(rest) = low.strip_prefix("# escape=").or_else(|| low.strip_prefix("#escape=")) {
            let ch = rest.trim();
            if ch != "\\" {
                return Err(AppError::BadRequest(
                    "parser directive `# escape=` の変更は使えません(既定の `\\` のみ)".into(),
                ));
            }
        }
    }
    let mut from_count = 0usize;
    for logical in logical_lines(text) {
        let trimmed = logical.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue; // 空行 / コメント / parser directive
        }
        let instr = trimmed
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        if instr == "COPY" || instr == "ADD" {
            return Err(AppError::BadRequest(format!(
                "'{instr}' はサーバ側ビルドでは使えません(コンテキスト無しの契約)。\
                 ファイルをイメージに入れたい場合は GitHub 経路か tbm deploy --local を使ってください"
            )));
        }
        if !ALLOWED_INSTRUCTIONS.contains(&instr.as_str()) {
            return Err(AppError::BadRequest(format!(
                "'{instr}' はサーバ側ビルドでは使えません(許可: {})",
                ALLOWED_INSTRUCTIONS.join(" ")
            )));
        }
        if instr == "FROM" {
            from_count += 1;
            if from_count > 1 {
                return Err(AppError::BadRequest(
                    "FROM は 1 回だけにしてください(COPY 不可のため multi-stage は使えません)"
                        .into(),
                ));
            }
        }
    }
    if from_count == 0 {
        return Err(AppError::BadRequest("Dockerfile に FROM がありません".into()));
    }
    Ok(())
}

/// 行継続 `\`(行末バックスラッシュ)を畳んで論理行を返す。素朴な結合で足りる —
/// 白名単判定に使うのは**先頭語だけ**で、RUN の中身の複雑さ(ヒアドキュメント等)は
/// 先頭語が RUN である時点で許可済み。ヒアドキュメント本文の行が独立の論理行に見えても、
/// 未知命令として**拒否側に倒れる**(fail-closed。必要になったら緩める)。
fn logical_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        let trimmed_end = line.trim_end();
        // コメント行(継続の途中でない時)は独立行 — Docker はコメントを `\` 継続しない
        // (`# escape=\` 等のディレクティブが次の命令に畳み込まれて隠すのを防ぐ)。
        if cur.is_empty() && trimmed_end.trim_start().starts_with('#') {
            out.push(trimmed_end.to_string());
            continue;
        }
        if let Some(stripped) = trimmed_end.strip_suffix('\\') {
            cur.push_str(stripped);
            cur.push(' ');
        } else {
            cur.push_str(trimmed_end);
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// イメージ参照の素朴な検証(registry が最終判定するので形だけ:空白・制御文字・過長を弾く)。
/// `[` `]` は IPv6 registry ホスト(`[2001:db8::1]:5000/repo`)のため許容(codex 指摘)。
fn validate_image_ref(s: &str) -> AppResult<()> {
    let ok = !s.is_empty()
        && s.len() <= MAX_IMAGE_REF_LEN
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'@' | b'-' | b'[' | b']')
        });
    if ok {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "イメージ参照が不正です(例: pgvector/pgvector:pg17)".into(),
        ))
    }
}

/// 参照(または FROM 基底)の registry ホスト部を取り出す。docker の規約:先頭の `/` より前が
/// **`.` か `:` を含む、または `localhost`** ならホスト、そうでなければ暗黙の docker.io。
/// 戻りは小文字化したホスト(port 込み)。ホスト無し(docker.io)は None。
fn registry_host_of(image_ref: &str) -> Option<String> {
    let first = image_ref.split('/').next().unwrap_or("");
    if first == "localhost" || first.contains('.') || first.contains(':') {
        Some(first.to_ascii_lowercase())
    } else {
        None
    }
}

/// この参照が **内部 registry / loopback を指していないか**を検証する。deploy-source は
/// サーバ側(host netns・内部 registry は無認証)で pull/FROM を解決するので、他テナントの
/// service UUID を狙った `<registry_pull>/<victim-uuid>:tag` で越境読み取りできてしまう
/// (CI/--local の push は自 repo・digest ピン留めで、この読み取り経路は無かった)。
/// 設定済みの registry ホスト(pull/push/direct)と loopback を拒否する。
fn reject_internal_registry(state: &AppState, image_ref: &str) -> AppResult<()> {
    let Some(host) = registry_host_of(image_ref) else {
        return Ok(()); // docker.io 等の暗黙ホストは対象外
    };
    let host_only = host.split(':').next().unwrap_or(&host);
    let is_loopback = host_only == "localhost"
        || host_only == "::1"
        || host_only == "[::1]"
        || host_only.starts_with("127.");
    let cfg = &state.config;
    let internal = [
        Some(cfg.registry_pull.to_ascii_lowercase()),
        Some(cfg.registry_push.to_ascii_lowercase()),
        cfg.registry_direct.as_ref().map(|s| s.to_ascii_lowercase()),
    ];
    let matches_internal = internal.iter().flatten().any(|h| {
        // host:port 完全一致 か host 部一致(port 省略のケース)。
        *h == host || h.split(':').next() == Some(host_only)
    });
    if is_loopback || matches_internal {
        return Err(AppError::BadRequest(
            "内部 registry / loopback を指すイメージ参照は使えません(他サービスのイメージは取得できません)".into(),
        ));
    }
    Ok(())
}

/// Dockerfile の(唯一の)FROM の基底イメージ参照を取り出す。`FROM [--platform=…] <image> [AS x]`。
/// validate_dockerfile 通過後に呼ぶ前提(FROM は 1 個)。見つからなければ None。
fn first_from_base(dockerfile: &str) -> Option<String> {
    for logical in logical_lines(dockerfile) {
        let t = logical.trim();
        let mut it = t.split_whitespace();
        if it.next().map(|w| w.eq_ignore_ascii_case("FROM")) != Some(true) {
            continue;
        }
        // `--platform=…` 等のフラグを飛ばし、最初の非フラグを基底とする。
        return it.find(|w| !w.starts_with("--")).map(str::to_string);
    }
    None
}

/// 合成 sha:sha256(kind ‖ NUL ‖ spec) の先頭 12 hex。**純 hex 必須** — CLI の
/// `looks_like_sha` / `sha_matches`(verify --for-sha の待機)と互換にするため。
/// 人間可読の見出しは commit_message 側(source_label)。
fn synth_git_sha(kind: &str, spec: &str) -> String {
    let mut h = Sha256::new();
    h.update(kind.as_bytes());
    h.update([0u8]);
    h.update(spec.as_bytes());
    hex::encode(h.finalize())[..12].to_string()
}

/// deploys.commit_message に入れる見出し(web / CLI の履歴表示用)。
/// image → "image: <ref>" / dockerfile → "dockerfile: <FROM 行>"。500 字で切る
/// (deploy.rs::sanitize_commit_message と同じ上限・char 境界安全)。
fn source_label(kind: &str, spec: &str) -> String {
    let body = if kind == SOURCE_KIND_DOCKERFILE {
        logical_lines(spec)
            .into_iter()
            .find(|l| l.trim_start().to_ascii_uppercase().starts_with("FROM"))
            .map(|l| l.trim().to_string())
            .unwrap_or_default()
    } else {
        spec.to_string()
    };
    format!("{kind}: {body}").chars().take(500).collect()
}

/// 単一エントリ "Dockerfile" の in-memory tar(コンテキスト無しビルドの本体)。
/// mtime 0 / mode 0o644 で決定的にする(内容が同じなら tar も同じ)。
fn dockerfile_tar(text: &str) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    let bytes = text.as_bytes();
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(0);
    header.set_cksum();
    // in-memory の Vec への書き込みは失敗しない(サイズも header と一致)。
    builder
        .append_data(&mut header, "Dockerfile", bytes)
        .expect("in-memory tar append");
    builder.into_inner().expect("in-memory tar finish")
}

/// `POST /api/services/{id}/deploy-source`(Bearer / session)。検証と deploys 行の作成まで
/// 同期で行い **202 即返し**、取得(pull / build / push)〜起動は spawn したタスクが担う。
/// 完走待ちは CLI 側(`tbm service verify --wait --for-sha <返した git_sha>`)。
pub async fn deploy_source(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<DeploySourceReq>,
) -> AppResult<(StatusCode, Json<DeploySourceResp>)> {
    ensure_owned(&state, auth.user_id, id).await?;
    // 形式検証 + 内部 registry / loopback を指す参照の拒否(越境読み取り防止)。
    match req.kind.as_str() {
        SOURCE_KIND_IMAGE => {
            validate_image_ref(&req.spec)?;
            reject_internal_registry(&state, &req.spec)?;
        }
        SOURCE_KIND_DOCKERFILE => {
            validate_dockerfile(&req.spec)?;
            if let Some(base) = first_from_base(&req.spec) {
                reject_internal_registry(&state, &base)?;
            }
        }
        _ => {
            return Err(AppError::BadRequest(
                "kind は image / dockerfile のいずれかにしてください".into(),
            ));
        }
    }
    let git_sha = synth_git_sha(&req.kind, &req.spec);
    let label = source_label(&req.kind, &req.spec);

    // deploys 行の作成 + 配方保存を **1 トランザクション**で(片方だけ commit されて
    // 宙吊り行が残るのを防ぐ)。同時に **同 service に進行中のデプロイが無いこと**を課す:
    // 取得は分単位になり得るので、無制限に並行 spawn すると Pi が飽和する + 同 tag への
    // 並行 push が競合する(codex/efficiency 指摘)。1 service = 同時 1 デプロイに制限する。
    let mut tx = state.db.begin().await?;
    let inflight: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM deploys
           WHERE service_id = $1 AND status IN ('received','pulling','starting'))",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if inflight {
        return Err(AppError::Conflict(
            "この service は既にデプロイが進行中です。完了を待って再実行してください(`tbm service deploys` で確認)".into(),
        ));
    }
    let deploy_id: Uuid = sqlx::query_scalar(
        "INSERT INTO deploys (service_id, git_sha, image_digest, status, commit_message)
              VALUES ($1, $2, $3, 'received', $4) RETURNING id",
    )
    .bind(id)
    .bind(&git_sha)
    .bind(PLACEHOLDER_DIGEST)
    .bind(&label)
    .fetch_one(&mut *tx)
    .await?;
    // 配方を provenance(最後に使った source)として保存。※完全な期望状態ではない —
    // hook/--local 経路は書き戻さないので、経路 1/2 に戻ると値は前回の source-deploy のまま残る。
    sqlx::query("UPDATE service_details SET source_kind = $2, source_spec = $3 WHERE resource_id = $1")
        .bind(id)
        .bind(&req.kind)
        .bind(&req.spec)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    audit(
        &state.db,
        Some(auth.user_id),
        "service.deploy_source",
        id,
        json!({
            "kind": req.kind,
            "spec": req.spec.chars().take(500).collect::<String>(),
            "git_sha": git_sha,
        }),
        auth.client_ip.as_deref(),
    )
    .await;

    // 非同期パイプラインへ(hook と同じ panic 包囲。deploy.rs:176 の作法)。req を move して
    // clone を避ける(spec は最大 8KiB)。
    let state2 = state.clone();
    let git_sha2 = git_sha.clone();
    let DeploySourceReq { kind, spec } = req;
    tokio::spawn(async move {
        let outcome = AssertUnwindSafe(acquire_and_deploy(
            &state2, deploy_id, id, &kind, &spec, &git_sha2,
        ))
        .catch_unwind()
        .await;
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::error!(error = ?e, %deploy_id, service_id = %id, "deploy-source パイプライン失敗")
            }
            Err(_) => {
                tracing::error!(%deploy_id, service_id = %id, "deploy-source タスクが panic");
                fail_acquire(&state2, deploy_id, id, "内部エラー(panic)").await;
            }
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(DeploySourceResp {
            service_id: id,
            deploy_id,
            git_sha,
        }),
    ))
}

/// 取得段階の失敗を記録する(mark_failed とは違い **phase は条件付き**で更新する)。
/// acquire は deploy_lock の外で分単位走るので、その間に別の hook デプロイが成功して
/// phase='running' になっていることがある。無条件に phase='failed' にすると健全に serving
/// している app を誤って停止扱いにする(v48 審査②と同型の穴)ため、phase は
/// **まだ 'deploying'(=この取得が最新)なら**だけ落とす。deploys 行は自分の行なので無条件。
async fn fail_acquire(state: &AppState, deploy_id: Uuid, service_id: Uuid, msg: &str) {
    let _ = sqlx::query(
        "UPDATE deploys SET status='failed', error=$2, finished_at=now()
           WHERE id=$1 AND status NOT IN ('succeeded','failed')",
    )
    .bind(deploy_id)
    .bind(msg)
    .execute(&state.db)
    .await;
    let _ = sqlx::query(
        "UPDATE service_details SET phase='failed', phase_detail=$2
           WHERE resource_id=$1 AND phase='deploying'",
    )
    .bind(service_id)
    .bind(msg)
    .execute(&state.db)
    .await;
}

/// 取得(pull / build → 内部 registry へ push → digest 確定)→ 既存 run_digest への合流。
/// 取得中の失敗は fail_acquire(deploys=failed。phase は条件付き)。旧コンテナは無傷で serving 継続。
async fn acquire_and_deploy(
    state: &AppState,
    deploy_id: Uuid,
    service_id: Uuid,
    kind: &str,
    spec: &str,
    git_sha: &str,
) -> AppResult<()> {
    // **最初に phase=deploying を立てる**:取得中に server が再起動しても、起動時の
    // recover_interrupted(phase='deploying' を掃く)がこの deploy 行を failed に収束させる。
    // これが無いと取得中クラッシュで deploys 行が非 terminal のまま永久に残る。
    let _ = sqlx::query(
        "UPDATE service_details SET phase='deploying', phase_detail=NULL WHERE resource_id=$1",
    )
    .bind(service_id)
    .execute(&state.db)
    .await;
    deploy::set_status(state, deploy_id, "pulling").await;

    // registry tag は **deploy_id** で一意化する(合成 git_sha は表示/待機用。同 tag への
    // 並行 push で別 deploy の manifest を読む競合を根絶する — codex 指摘)。
    let tag = deploy_id.simple().to_string();
    let acquired = async {
        match kind {
            SOURCE_KIND_IMAGE => {
                docker::pull_external(state, spec).await?;
                docker::push_to_internal(state, spec, service_id, &tag).await?;
            }
            SOURCE_KIND_DOCKERFILE => {
                let internal_ref =
                    format!("{}/{}:{}", state.config.registry_pull, service_id, tag);
                docker::build_dockerfile(state, dockerfile_tar(spec), &internal_ref).await?;
                docker::push_to_internal(state, &internal_ref, service_id, &tag).await?;
            }
            _ => {
                // handler で検証済み。ここに来たら実装バグ。
                return Err(AppError::Other(anyhow!("未知の source kind: {kind}")));
            }
        }
        registry::pushed_manifest_digest(state, service_id, &tag).await
    }
    .await;
    let digest = match acquired {
        Ok(d) => d,
        Err(e) => {
            fail_acquire(state, deploy_id, service_id, &e.to_string()).await;
            return Err(e);
        }
    };

    // 取得中に世界が変わっていないかを確認する。acquire は deploy_lock の外で分単位走るので、
    // その間に別デプロイの完了 / stop / delete が起き得る(codex 指摘)。判定は **phase**:
    // 取得開始時に自分で 'deploying' を立てたので、**まだ 'deploying' なら自分が最新の取得**。
    // それ以外(stop→'stopped' / 先に完了した別デプロイ→'running' / 初回未起動なら我々が
    // 'deploying' に上書き済み)は、我々が上書きすべきでない = 中止する。行が無ければ削除済み。
    // 完全な排他は run_digest がロック下で行う(ここは大半の窓を潰す前段)。
    let cur_phase: Option<String> = sqlx::query_scalar(
        "SELECT s.phase FROM service_details s
           JOIN resources r ON r.id = s.resource_id
          WHERE s.resource_id = $1 AND r.kind = 'service' AND r.deleted_at IS NULL",
    )
    .bind(service_id)
    .fetch_optional(&state.db)
    .await?;
    match cur_phase.as_deref() {
        None => {
            fail_acquire(state, deploy_id, service_id, "取得完了時に service が削除されていました").await;
            return Ok(());
        }
        Some("deploying") => {} // 我々が最新の取得 — 続行
        Some(_) => {
            fail_acquire(
                state,
                deploy_id,
                service_id,
                "取得中に service の状態が変わりました(stop / 別デプロイ完了)。中止しました",
            )
            .await;
            return Ok(());
        }
    }

    // digest 確定 → deploys 行を本物に更新してから既存パイプラインへ(rollback / GC は
    // この行を通常の deploy と同様に扱える)。**この UPDATE は digest 不変量を運ぶので必須**
    // (失敗を握り潰すと succeeded なのに image_digest='pending' の行が残り、rollback / reconcile /
    // start が永久に壊れる — altitude/codex 指摘)。失敗したら fail_acquire で閉じる。
    if let Err(e) = sqlx::query("UPDATE deploys SET image_digest=$2 WHERE id=$1")
        .bind(deploy_id)
        .bind(&digest)
        .execute(&state.db)
        .await
    {
        fail_acquire(state, deploy_id, service_id, &format!("digest の記録に失敗: {e}")).await;
        return Err(AppError::Other(anyhow!("deploys.image_digest の更新に失敗: {e}")));
    }
    deploy::run_digest(
        state,
        deploy_id,
        service_id,
        &digest,
        git_sha,
        DeployTrigger::User,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dockerfile_accepts_basic() {
        let df = "# comment\nFROM alpine:3.20\nRUN apk add --no-cache curl\nCMD [\"sleep\", \"infinity\"]\n";
        assert!(validate_dockerfile(df).is_ok());
    }

    #[test]
    fn dockerfile_accepts_line_continuation_and_lowercase() {
        // 小文字命令 + 行継続(継続行の先頭語は命令として判定しない)。
        let df = "from alpine:3.20\nrun apk add --no-cache \\\n    curl \\\n    jq\n";
        assert!(validate_dockerfile(df).is_ok());
    }

    #[test]
    fn dockerfile_accepts_parser_directive() {
        assert!(validate_dockerfile("# syntax=docker/dockerfile:1\nFROM alpine\n").is_ok());
    }

    #[test]
    fn dockerfile_rejects_copy_add_volume_onbuild() {
        for bad in [
            "FROM alpine\nCOPY . /app\n",
            "FROM alpine\nADD http://x/y /y\n",
            "FROM alpine\nVOLUME /data\n",
            "FROM alpine\nONBUILD RUN true\n",
        ] {
            assert!(validate_dockerfile(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn dockerfile_rejects_unknown_and_heredoc_body() {
        // 未知命令は拒否(fail-closed)。ヒアドキュメント本文も未知命令に見え拒否側に倒れる。
        assert!(validate_dockerfile("FROM alpine\nFOOBAR x\n").is_err());
        assert!(
            validate_dockerfile("FROM alpine\nRUN <<EOF\necho hi\nEOF\n").is_err(),
            "heredoc body lines fail closed"
        );
    }

    #[test]
    fn dockerfile_rejects_multi_from_no_from_empty_oversize() {
        assert!(validate_dockerfile("FROM a\nFROM b\n").is_err());
        assert!(validate_dockerfile("RUN true\n").is_err());
        assert!(validate_dockerfile("   \n").is_err());
        let big = format!("FROM alpine\n{}", "ENV A=B\n".repeat(2000));
        assert!(big.len() > MAX_DOCKERFILE_BYTES);
        assert!(validate_dockerfile(&big).is_err());
    }

    #[test]
    fn image_ref_validation() {
        for ok in [
            "nginx",
            "pgvector/pgvector:pg17",
            "ghcr.io/a/b:v1",
            "registry.example.com:5000/a/b@sha256:0123456789abcdef",
            "[2001:db8::1]:5000/repo:tag", // IPv6 registry(codex 指摘)
        ] {
            assert!(validate_image_ref(ok).is_ok(), "should accept: {ok}");
        }
        for bad in ["", "a b", "a\nb", &"x".repeat(MAX_IMAGE_REF_LEN + 1)] {
            assert!(validate_image_ref(bad).is_err(), "should reject: {bad:?}");
        }
    }

    #[test]
    fn dockerfile_rejects_nonstandard_escape_directive() {
        // `# escape=\`` を許すと logical_lines の `\` 畳みが Docker とズレて COPY をすり抜けさせ得る。
        assert!(validate_dockerfile("# escape=`\nFROM alpine\nRUN true\n").is_err());
        // 既定の `\` を明示するのは許容。
        assert!(validate_dockerfile("# escape=\\\nFROM alpine\nRUN true\n").is_ok());
    }

    #[test]
    fn registry_host_extraction() {
        assert_eq!(registry_host_of("nginx"), None);
        assert_eq!(registry_host_of("pgvector/pgvector:pg17"), None);
        assert_eq!(
            registry_host_of("127.0.0.1:5000/uuid:tag").as_deref(),
            Some("127.0.0.1:5000")
        );
        assert_eq!(
            registry_host_of("ghcr.io/a/b:v1").as_deref(),
            Some("ghcr.io")
        );
        assert_eq!(
            registry_host_of("localhost/x").as_deref(),
            Some("localhost")
        );
    }

    #[test]
    fn from_base_extraction() {
        assert_eq!(
            first_from_base("FROM alpine:3.20\nRUN true\n").as_deref(),
            Some("alpine:3.20")
        );
        assert_eq!(
            first_from_base("# c\nFROM --platform=linux/arm64 debian:bookworm AS base\n").as_deref(),
            Some("debian:bookworm")
        );
        assert_eq!(first_from_base("RUN true\n"), None);
    }

    #[test]
    fn synth_git_sha_is_pure_hex_and_deterministic() {
        let a = synth_git_sha("image", "nginx:latest");
        assert_eq!(a.len(), 12);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(a, synth_git_sha("image", "nginx:latest"));
        // kind が違えば別 sha(NUL 区切りで境界も曖昧にならない)。
        assert_ne!(a, synth_git_sha("dockerfile", "nginx:latest"));
    }

    #[test]
    fn source_label_shapes() {
        assert_eq!(source_label("image", "nginx:1"), "image: nginx:1");
        assert_eq!(
            source_label("dockerfile", "# c\nFROM alpine:3.20\nRUN true\n"),
            "dockerfile: FROM alpine:3.20"
        );
        // 500 字 char 境界切り(マルチバイトで panic しない)。
        let long = format!("あ{}", "x".repeat(600));
        assert_eq!(source_label("image", &long).chars().count(), 500);
    }

    #[test]
    fn dockerfile_tar_roundtrip() {
        let text = "FROM alpine\nRUN true\n";
        let tar_bytes = dockerfile_tar(text);
        let mut ar = tar::Archive::new(&tar_bytes[..]);
        let mut entries = ar.entries().unwrap();
        let mut entry = entries.next().unwrap().unwrap();
        assert_eq!(entry.path().unwrap().to_str().unwrap(), "Dockerfile");
        let mut content = String::new();
        std::io::Read::read_to_string(&mut entry, &mut content).unwrap();
        assert_eq!(content, text);
        assert!(entries.next().is_none());
    }
}
