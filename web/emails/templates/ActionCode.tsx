import { Section, Text } from "@react-email/components";

import { Badge, C, heading, Layout, paragraph } from "../components/Layout";

// owner 危険操作の確認コード。変数:{{code}} / {{kind}} / {{action}} / {{ttl}}。
// Rust 側 admin/actions.rs から送る。
export default function ActionCode() {
  return (
    <Layout preview="tsubomi: 確認コード">
      <Badge color={C.mint}>確認が必要です</Badge>
      <Text style={heading}>危険な操作の確認コード</Text>
      <Text style={paragraph}>
        owner 操作の確認コードです。下のコードを画面に入力すると、対象の{" "}
        <strong>{"{{kind}}"}</strong> を <strong>{"{{action}}"}</strong> します。
      </Text>
      <Section style={{ textAlign: "center", margin: "8px 0" }}>
        <Text
          style={{
            display: "inline-block",
            backgroundColor: "#e6f9f6",
            border: `2px solid ${C.mint}`,
            borderRadius: "12px",
            color: C.heading,
            fontFamily: '"SFMono-Regular", Consolas, "Liberation Mono", Menlo, monospace',
            fontSize: "30px",
            fontWeight: 700,
            letterSpacing: "8px",
            padding: "14px 24px",
            margin: 0,
          }}
        >
          {"{{code}}"}
        </Text>
      </Section>
      <Text style={{ ...paragraph, color: C.muted, fontSize: "13px", margin: 0 }}>
        有効期限 {"{{ttl}}"} 分。心当たりがなければ、このメールは無視してください
        (コードを他人に教えないでください)。
      </Text>
    </Layout>
  );
}
