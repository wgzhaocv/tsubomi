//! `.github/workflows/tsubomi-deploy.yml` のテンプレート(平台が単一真源として配る)。
//!
//! 平台は GitHub に一切触れない:このテンプレを service create のレスポンスで返し、
//! CLI(ユーザの gh)/ web(コピペ)がユーザの repo に置く。テンプレは gh の
//! `vars` / `secrets` を参照するので、service ごとの展開は不要な**静的テキスト**
//! (service_id / registry / hook_url / platforms はすべて gh variable で渡る)。
//!
//! 流れ(paas-m3-design §5.5 / §6):buildx で `TSUBOMI_PLATFORMS` の arch だけ build
//! → registry へ push → manifest digest を捕まえる → 生 body に HMAC-SHA256 を付けて
//! hook を叩く(ts ± 300s + nonce でリプレイ防御。HMAC = 送る生バイトそのもの)。

/// `tsubomi-deploy.yml` の中身。CLI / web がそのままファイルに書く。
pub const TEMPLATE: &str = r##"name: tsubomi deploy
on:
  # main / master どちらの既定ブランチでも起動する(既存 repo が master のこともある)。
  push: { branches: [main, master] }
  # シークレット修正後などに手動で再デプロイできるよう(空コミット不要)。
  workflow_dispatch: {}
jobs:
  deploy:
    # ランナーは gh variable TSUBOMI_RUNNER で決まる(service create 時に平台が platforms から
    # 導出して設定。arm64 のみ → ubuntu-24.04-arm 原生 = Rust 等のビルドが QEMU 比で桁違いに速い)。
    # 変数が無い古い repo は ubuntu-latest(amd64 + QEMU)へフォールバック。手動切替も
    # `gh variable set TSUBOMI_RUNNER --body ubuntu-24.04-arm` だけ(yml は不変)。
    runs-on: ${{ vars.TSUBOMI_RUNNER || 'ubuntu-latest' }}
    steps:
      - uses: actions/checkout@v4
      - uses: docker/setup-qemu-action@v3
      - uses: docker/setup-buildx-action@v3
      - uses: docker/login-action@v3
        with:
          registry: ${{ vars.TSUBOMI_REGISTRY }}
          username: ${{ secrets.TSUBOMI_REGISTRY_USER }}
          password: ${{ secrets.TSUBOMI_REGISTRY_PASS }}
      # build:Dockerfile があればそれ、無ければ nixpacks。--platform は平台が公布する
      # arch(既定 linux/arm64)。Dockerfile 経路は GHA 層キャッシュで再 build 数十秒
      # (nixpacks 経路はキャッシュ無しで毎回フル build)。
      - id: build
        run: |
          IMAGE=${{ vars.TSUBOMI_REGISTRY }}/${{ vars.TSUBOMI_SERVICE_ID }}:${{ github.sha }}
          if [ -f Dockerfile ]; then
            docker buildx build --platform "${{ vars.TSUBOMI_PLATFORMS }}" \
              --cache-from type=gha --cache-to type=gha,mode=max \
              --push -t "$IMAGE" --metadata-file meta.json .
            DIGEST=$(jq -r '."containerimage.digest"' meta.json)
          else
            # Dockerfile が無ければ nixpacks。npm 配布は無い(npx 経由は 404)ので公式インストーラで
            # CLI を入れる。nixpacks は単一 build で多 arch を作れない(公式仕様)ため、
            # arch 毎に build → push し docker manifest で 1 つの IMAGE に集約する(単一 arch なら 1 回)。
            curl -fsSL https://nixpacks.com/install.sh | bash
            REFS=""
            for P in $(echo "${{ vars.TSUBOMI_PLATFORMS }}" | tr ',' ' '); do
              ARCH_IMAGE="$IMAGE-${P//\//-}"
              nixpacks build . --name "$ARCH_IMAGE" --platform "$P"
              docker push "$ARCH_IMAGE"
              REFS="$REFS $ARCH_IMAGE"
            done
            docker manifest create "$IMAGE" $REFS
            docker manifest push "$IMAGE"
            DIGEST=$(docker buildx imagetools inspect "$IMAGE" --format '{{json .Manifest.Digest}}' | tr -d '"')
          fi
          # digest が空なら hook は必ず 400 になる。原因が分かるよう CI 側で先に止める。
          [ -n "$DIGEST" ] || { echo "image digest を取得できませんでした" >&2; exit 1; }
          echo "digest=$DIGEST" >> "$GITHUB_OUTPUT"
      - name: notify tsubomi
        run: |
          # commit の件名(deploy 履歴の見出し)。checkout 済みなので git log が使える
          # (workflow_dispatch でも HEAD の件名が取れる)。jq --arg なので引用符/改行は安全。
          BODY=$(jq -nc --arg s "${{ vars.TSUBOMI_SERVICE_ID }}" \
            --arg sha "${{ github.sha }}" --arg d "${{ steps.build.outputs.digest }}" \
            --arg msg "$(git log -1 --pretty=%s)" \
            --argjson ts "$(date +%s)" --arg n "$(openssl rand -hex 16)" \
            '{service_id:$s, git_sha:$sha, image_digest:$d, commit_message:$msg, ts:$ts, nonce:$n}')
          SIG=$(printf '%s' "$BODY" | openssl dgst -sha256 -hmac "${{ secrets.TSUBOMI_DEPLOY_KEY }}" -hex | sed 's/^.* //')
          curl -fsS -X POST "${{ vars.TSUBOMI_HOOK_URL }}" \
            -H "content-type: application/json" -H "x-tsubomi-signature: $SIG" -d "$BODY"
"##;

use tsubomi_shared::{RegistryCreds, ServiceDto, WORKFLOW_PATH};

/// `TSUBOMI_PLATFORMS`(buildx の build 対象)から GHA ランナーを導出する。
/// arm64 のみの部署なら原生 arm ランナー(QEMU の Rust ビルド数十分 → 原生数分)。
/// amd64 を含む(または不明な)場合は ubuntu-latest に倒す — 混在は結局どちらかを
/// QEMU で作るので、可用性が最も高い既定を選ぶ。個人私有 repo でも arm ランナーは
/// 無料枠で使える(2026-07 実機確認)ため arm 単独は原生を既定にできる。
pub fn runner_for(platforms: &str) -> &'static str {
    let mut archs = platforms
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .peekable();
    if archs.peek().is_some() && archs.all(|p| p == "linux/arm64") {
        "ubuntu-24.04-arm"
    } else {
        "ubuntu-latest"
    }
}

/// GitHub 連携の手順コマンド列(ユーザがリポジトリ直下で実行 / AI が実行 / web が表示)。
/// 平台が **単一真源**として組み立て、CreateServiceResp.setup_commands に載せる
/// (CLI / web はこの文字列をそのまま使い、各々で gh コマンドを再構築しない)。
///
/// 安全のための 2 点(CLI の自動実行路径 commands/service.rs と揃える):
/// - **`-R "$TSUBOMI_REPO"` を全 gh コマンドに付ける**:カレントが別の GitHub repo でも、
///   secret/variable が必ず新しい tsubomi repo に書かれる(既存 repo への誤書込み防止)。
/// - **secret は `printf | gh secret set` の stdin 渡し**(argv に値を載せない = `gh`
///   プロセス引数として ps から見えない)。`--body <secret>` は使わない。
///   ※ 値は乱数 base64url / uuid / DNS slug なので単引用符で安全に括れる。
pub fn setup_commands(
    service: &ServiceDto,
    deploy_key: &str,
    registry: &RegistryCreds,
    hook_url: &str,
    platforms: &str,
) -> Vec<String> {
    let sub = &service.subdomain;
    vec![
        // repo を `owner/sub` に固定してから create する(CLI 自動経路の `gh repo create {owner}/{sub}`
        // と一致 = 紛らわしい不一致を解消)。以降の gh も必ずこの新しい tsubomi repo を対象にする。
        format!("TSUBOMI_REPO=\"$(gh api user -q .login)/{sub}\""),
        format!("gh repo create \"$TSUBOMI_REPO\" --private --source=. --remote=tsubomi"),
        format!("printf %s '{deploy_key}' | gh secret set TSUBOMI_DEPLOY_KEY -R \"$TSUBOMI_REPO\""),
        format!(
            "printf %s '{}' | gh secret set TSUBOMI_REGISTRY_USER -R \"$TSUBOMI_REPO\"",
            registry.user
        ),
        format!(
            "printf %s '{}' | gh secret set TSUBOMI_REGISTRY_PASS -R \"$TSUBOMI_REPO\"",
            registry.pass
        ),
        format!(
            "gh variable set TSUBOMI_SERVICE_ID -R \"$TSUBOMI_REPO\" --body '{}'",
            service.id
        ),
        format!(
            "gh variable set TSUBOMI_REGISTRY -R \"$TSUBOMI_REPO\" --body '{}'",
            registry.host
        ),
        format!("gh variable set TSUBOMI_HOOK_URL -R \"$TSUBOMI_REPO\" --body '{hook_url}'"),
        format!("gh variable set TSUBOMI_PLATFORMS -R \"$TSUBOMI_REPO\" --body '{platforms}'"),
        format!(
            "gh variable set TSUBOMI_RUNNER -R \"$TSUBOMI_REPO\" --body '{}'",
            runner_for(platforms)
        ),
        format!(
            "# {WORKFLOW_PATH} を workflow_yaml の内容で作成 → git add/commit/push で自動デプロイ"
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::{TEMPLATE, runner_for};

    /// platforms → ランナー導出の真理値表(arm64 単独だけが原生 arm)。
    #[test]
    fn runner_derivation() {
        assert_eq!(runner_for("linux/arm64"), "ubuntu-24.04-arm");
        assert_eq!(runner_for("linux/arm64,linux/arm64"), "ubuntu-24.04-arm");
        assert_eq!(runner_for("linux/amd64"), "ubuntu-latest");
        assert_eq!(runner_for("linux/arm64,linux/amd64"), "ubuntu-latest");
        // 空 / 不正は可用性優先で ubuntu-latest。
        assert_eq!(runner_for(""), "ubuntu-latest");
        assert_eq!(runner_for(" , "), "ubuntu-latest");
    }

    /// テンプレが hook 契約の必須要素を持つことを固定する(占位の取りこぼし防止)。
    #[test]
    fn template_has_required_pieces() {
        for needle in [
            "vars.TSUBOMI_REGISTRY",
            "vars.TSUBOMI_SERVICE_ID",
            "vars.TSUBOMI_HOOK_URL",
            "vars.TSUBOMI_PLATFORMS",
            // ランナーは gh variable で切替(平台が platforms から導出。yml は不変のまま)。
            "vars.TSUBOMI_RUNNER",
            "secrets.TSUBOMI_DEPLOY_KEY",
            "secrets.TSUBOMI_REGISTRY_USER",
            "secrets.TSUBOMI_REGISTRY_PASS",
            "x-tsubomi-signature",
            "image_digest",
            // commit の件名を hook body に載せる(deploy 履歴の見出し)。
            "commit_message",
            // 手動再デプロイ(空コミット不要)。
            "workflow_dispatch",
        ] {
            assert!(
                TEMPLATE.contains(needle),
                "workflow テンプレに {needle} が無い"
            );
        }
    }

    /// 壊れた nixpacks 配方(存在しない npm パッケージ)が復活しないことを固定する。
    /// nixpacks は公式インストーラで入れ、`--push` flag は持たない(per-arch + manifest 集約)。
    #[test]
    fn template_uses_official_nixpacks_installer() {
        assert!(
            !TEMPLATE.contains("@railway/nixpacks"),
            "存在しない npm パッケージ @railway/nixpacks を使っている(404)"
        );
        assert!(
            TEMPLATE.contains("https://nixpacks.com/install.sh"),
            "nixpacks の公式インストーラを使っていない"
        );
    }
}
