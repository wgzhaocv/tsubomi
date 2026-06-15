import { Section, Text } from "@react-email/components";

import { Badge, C, heading, Layout, paragraph } from "../components/Layout";

// 磁盘水位告警。変数:{{accent}}(level 別の色を Rust が渡す)/ {{pct}} / {{level}} /
// {{warn}} / {{critical}} / {{path}}。Rust 側 gc.rs から送る。
export default function DiskAlert() {
  return (
    <Layout preview="tsubomi: ディスク使用率の警告">
      <Badge color="{{accent}}">⚠ ディスク警告 · {"{{level}}"}</Badge>
      <Text style={heading}>ディスク使用率が {"{{pct}}"}% に達しました</Text>
      <Text style={paragraph}>
        tsubomi のディスク使用率が <strong style={{ color: "{{accent}}" }}>{"{{pct}}"}%</strong>{" "}
        になりました。古いバックアップ / ゴミ箱の整理、不要な volume の削除、容量増設を
        検討してください。
      </Text>
      <Section
        style={{
          backgroundColor: "#ffffff",
          border: `1px solid ${C.border}`,
          borderRadius: "12px",
          padding: "14px 16px",
          margin: "8px 0 0",
        }}
      >
        <Text style={{ ...paragraph, margin: 0, fontSize: "13px" }}>
          しきい値:警告 {"{{warn}}"}% / 危険 {"{{critical}}"}%
          <br />
          監視パス:<span style={{ color: C.muted }}>{"{{path}}"}</span>
        </Text>
      </Section>
    </Layout>
  );
}
