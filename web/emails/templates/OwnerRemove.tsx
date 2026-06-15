import { Text } from "@react-email/components";

import { Badge, C, heading, Layout, paragraph } from "../components/Layout";

// owner 権限の解除通知。変数なし(静的)。Rust 側 owners.rs の remove から送る。
export default function OwnerRemove() {
  return (
    <Layout preview="tsubomi の owner 権限が解除されました">
      <Badge color={C.danger}>owner 権限の変更</Badge>
      <Text style={heading}>owner 権限が解除されました</Text>
      <Text style={paragraph}>
        あなたの tsubomi の <strong>owner(管理者)権限</strong> が解除されました。
        これ以降、管理画面の操作(他ユーザの資源の停止 / 削除、共有パスワード、IP
        許可リスト、owner 管理)は行えません。
      </Text>
      <Text style={{ ...paragraph, color: C.muted, fontSize: "13px", margin: 0 }}>
        必要であれば、別の owner にあらためて追加を依頼してください。心当たりがない場合は
        別の owner に確認してください。
      </Text>
    </Layout>
  );
}
