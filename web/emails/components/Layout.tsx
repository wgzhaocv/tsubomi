import {
  Body,
  Container,
  Head,
  Hr,
  Html,
  Img,
  Preview,
  Section,
  Text,
} from "@react-email/components";
import type { ReactNode } from "react";

// tsubomi(つぼみ)メールの共通フレーム。web の animal-island 風デザイン
// (_design-reference)を email-safe に落とす:インラインスタイルのみ、@font-face なし、
// box-shadow の代わりに border、pill ボタン、ロゴは本番ドメインの絶対 URL。
// 3 テンプレ(owner 解除 / 磁盘告警 / 検証コード)が本フレームを共有する。

export const C = {
  bg: "#f8f8f0",
  card: "#f7f3df",
  border: "#c4b89e",
  heading: "#794f27",
  body: "#725d42",
  muted: "#9f927d",
  mint: "#19c8b9",
  success: "#6fba2c",
  warning: "#f5c31c",
  danger: "#e05a5a",
} as const;

export const FONT =
  '"Segoe UI", Roboto, "Helvetica Neue", Arial, "Noto Sans JP", "Hiragino Kaku Gothic ProN", sans-serif';

const LOGO_URL = "https://tsubomi-app.com/logo.png";

export function Layout({ preview, children }: { preview: string; children: ReactNode }) {
  return (
    <Html lang="ja">
      <Head />
      <Preview>{preview}</Preview>
      <Body style={{ backgroundColor: C.bg, margin: 0, padding: "24px 0", fontFamily: FONT }}>
        <Container style={{ maxWidth: "600px", margin: "0 auto", padding: "0 16px" }}>
          <Section style={{ textAlign: "center", padding: "8px 0 20px" }}>
            <Img
              src={LOGO_URL}
              alt="tsubomi"
              width="132"
              style={{ display: "inline-block", maxWidth: "100%" }}
            />
          </Section>
          <Section
            style={{
              backgroundColor: C.card,
              border: `1px solid ${C.border}`,
              borderRadius: "16px",
              padding: "28px 24px",
            }}
          >
            {children}
          </Section>
          <Hr style={{ borderColor: C.border, opacity: 0.5, margin: "20px 0 12px" }} />
          <Text
            style={{
              color: C.muted,
              fontSize: "12px",
              textAlign: "center",
              lineHeight: 1.6,
              margin: 0,
            }}
          >
            つぼみ — 社内 PaaS プラットフォーム
            <br />
            このメールに心当たりがない場合は、別の管理者にご確認ください。
          </Text>
        </Container>
      </Body>
    </Html>
  );
}

// カード上部の pill 徽章(3 テンプレ共通)。色だけ差し替える。color は定数色のほか
// "{{accent}}"(Rust が level 別に置換するプレースホルダ)も渡せる(文字列を透過するだけ)。
export function Badge({ color, children }: { color: string; children: ReactNode }) {
  return (
    <Section
      style={{
        display: "inline-block",
        backgroundColor: color,
        color: "#ffffff",
        fontSize: "13px",
        fontWeight: 700,
        padding: "4px 14px",
        borderRadius: "999px",
        marginBottom: "16px",
      }}
    >
      {children}
    </Section>
  );
}

// 見出し / 本文 / ボタンの共通スタイル(テンプレ間で使い回す)。
export const heading = {
  color: C.heading,
  fontSize: "22px",
  fontWeight: 700,
  margin: "0 0 12px",
  lineHeight: 1.4,
} as const;

export const paragraph = {
  color: C.body,
  fontSize: "15px",
  fontWeight: 500,
  lineHeight: 1.7,
  margin: "0 0 12px",
} as const;
