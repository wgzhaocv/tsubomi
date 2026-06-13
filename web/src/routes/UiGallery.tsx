import { type ReactNode, useState } from "react";
import { Plus, Search, Star } from "lucide-react";

import { PageMeta } from "@/components/page-meta";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { CodeBlock } from "@/components/ui/codeblock";
import { Divider } from "@/components/ui/divider";
import { Footer } from "@/components/ui/footer";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { Radio } from "@/components/ui/radio";
import { Select } from "@/components/ui/select";
import { Tabs } from "@/components/ui/tabs";
import { Time } from "@/components/ui/time";
import { Title } from "@/components/ui/title";
import { Tooltip } from "@/components/ui/tooltip";
import { Typewriter } from "@/components/ui/typewriter";
import {
  IconCamera,
  IconChat,
  IconCritterpedia,
  IconDesign,
  IconDiy,
  IconHelicopter,
  IconMap,
  IconMiles,
  IconShopping,
  IconVariant,
} from "@/components/ui/icons";

// 開発用スタイル画廊:全 component の variant を一覧して目視確認する。
// 本番ルートではない(動森スタイル移植の検証・リファレンス用)。

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-2">
      <p className="text-xs font-bold text-muted-foreground">{label}</p>
      <div className="flex flex-wrap items-center gap-3">{children}</div>
    </div>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <Card className="w-full max-w-3xl">
      <CardHeader>
        <CardTitle className="text-xl">{title}</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-5">{children}</CardContent>
    </Card>
  );
}

export default function UiGallery() {
  const [open, setOpen] = useState(false);
  const [fruit, setFruit] = useState("");

  return (
    <main className="flex min-h-dvh flex-col items-center gap-6 p-8 pb-0 text-foreground">
      <PageMeta title="UI 画廊" />
      <h1 className="text-3xl font-extrabold tracking-tight">UI 画廊</h1>

      {/* ---------- Button ---------- */}
      <Section title="Button">
        <Row label="type 種類">
          <Button type="primary">Primary</Button>
          <Button type="default">Default</Button>
          <Button type="dashed">Dashed</Button>
          <Button type="text">Text</Button>
          <Button type="link">Link</Button>
        </Row>
        <Row label="danger / ghost / loading / disabled 状態">
          <Button type="primary" danger>
            Danger
          </Button>
          <Button ghost>Ghost</Button>
          <Button loading type="primary">
            Loading
          </Button>
          <Button disabled>Disabled</Button>
        </Row>
        <Row label="size 寸法">
          <Button size="small">Small</Button>
          <Button size="middle">Middle</Button>
          <Button size="large">Large</Button>
        </Row>
        <Row label="icon アイコン">
          <Button icon={<Search />}>検索</Button>
          <Button type="default" icon={<Star />}>
            お気に入り
          </Button>
          <Button type="dashed" icon={<Plus />}>
            追加
          </Button>
        </Row>
        <Row label="danger 組合">
          <Button type="primary" danger>
            Primary Danger
          </Button>
          <Button type="default" danger>
            Default Danger
          </Button>
          <Button type="dashed" danger>
            Dashed Danger
          </Button>
        </Row>
      </Section>

      {/* ---------- Title ---------- */}
      <Section title="Title(リボン)">
        <Row label="size 寸法">
          <Title size="small">Small</Title>
          <Title size="middle">つぼみ</Title>
          <Title size="large">Large</Title>
        </Row>
        <Row label="color 色板">
          <Title color="app-pink">Pink</Title>
          <Title color="app-blue">Blue</Title>
          <Title color="app-yellow">Yellow</Title>
          <Title color="app-red">Red</Title>
          <Title color="brown">Brown</Title>
        </Row>
      </Section>

      {/* ---------- Icon ---------- */}
      <Section title="Icon(tree-shakeable / NookPhone)">
        <Row label="個別 named export(未使用は tree-shake で落ちる)">
          <IconMiles size={32} />
          <IconCamera size={32} />
          <IconChat size={32} />
          <IconCritterpedia size={32} />
          <IconDesign size={32} />
          <IconDiy size={32} />
          <IconHelicopter size={32} />
          <IconMap size={32} />
          <IconShopping size={32} />
          <IconVariant size={32} />
        </Row>
      </Section>

      {/* ---------- Input ---------- */}
      <Section title="Input">
        <Row label="基本 / 影あり">
          <Input aria-label="名前(基本)" placeholder="名前を入力" />
          <Input aria-label="名前(影あり)" shadow placeholder="影あり(shadow)" />
        </Row>
        <Row label="prefix / suffix">
          <Input aria-label="検索" prefix={<Search />} placeholder="検索…" />
          <Input aria-label="金額" suffix="円" defaultValue="1000" />
        </Row>
        <Row label="allowClear / status / size / disabled">
          <Input aria-label="クリア可能" allowClear defaultValue="消せます" />
          <Input aria-label="エラー例" status="error" defaultValue="エラー" />
          <Input aria-label="警告例" status="warning" defaultValue="警告" />
          <Input aria-label="small サイズ" size="small" placeholder="small" />
          <Input aria-label="large サイズ" size="large" placeholder="large" />
          <Input aria-label="無効" disabled defaultValue="無効" />
        </Row>
      </Section>

      {/* ---------- Select / Checkbox / Radio ---------- */}
      <Section title="Select / Checkbox / Radio">
        <Row label="Select">
          <div className="w-56">
            <Select
              aria-label="果物を選択"
              placeholder="果物を選択"
              value={fruit}
              onChange={setFruit}
              options={[
                { key: "apple", label: "りんご" },
                { key: "orange", label: "みかん" },
                { key: "grape", label: "ぶどう" },
              ]}
            />
          </div>
        </Row>
        <Row label="Checkbox">
          <Checkbox
            defaultValue={["a"]}
            options={[
              { label: "オプション A", value: "a" },
              { label: "オプション B", value: "b" },
              { label: "無効", value: "c", disabled: true },
            ]}
          />
        </Row>
        <Row label="Radio">
          <Radio
            defaultValue="a"
            options={[
              { label: "はい", value: "a" },
              { label: "いいえ", value: "b" },
              { label: "無効", value: "c", disabled: true },
            ]}
          />
        </Row>
      </Section>

      {/* ---------- Tabs ---------- */}
      <Section title="Tabs">
        <Tabs
          defaultActiveKey="a"
          items={[
            { key: "a", label: "概要", children: <p>概要のタブ内容です。</p> },
            { key: "b", label: "設定", children: <p>設定のタブ内容です。</p> },
            { key: "c", label: "履歴", children: <p>履歴のタブ内容です。</p> },
          ]}
        />
      </Section>

      {/* ---------- Tooltip ---------- */}
      <Section title="Tooltip">
        <Row label="placement / variant(hover で表示)">
          <Tooltip title="上に表示" placement="top">
            <Button type="default">top</Button>
          </Tooltip>
          <Tooltip title="右に表示" placement="right">
            <Button type="default">right</Button>
          </Tooltip>
          <Tooltip title="下に表示" placement="bottom">
            <Button type="default">bottom</Button>
          </Tooltip>
          <Tooltip title="気泡スタイル" variant="island">
            <Button type="default">island</Button>
          </Tooltip>
        </Row>
      </Section>

      {/* ---------- Modal / Typewriter ---------- */}
      <Section title="Modal / Typewriter">
        <Row label="Modal(本文は打字機)">
          <Button
            type="primary"
            onClick={() => {
              setOpen(true);
            }}
          >
            モーダルを開く
          </Button>
        </Row>
        <Row label="Typewriter">
          <p className="text-sm">
            <Typewriter speed={45}>
              これは打字機エフェクトのテキストです。一文字ずつ表示されます。
            </Typewriter>
          </p>
        </Row>
      </Section>

      {/* ---------- CodeBlock ---------- */}
      <Section title="CodeBlock">
        <CodeBlock
          language="tsx"
          title="example.tsx"
          code={`function greet(name: string) {\n  return \`Hello, \${name}!\`;\n}`}
        />
      </Section>

      {/* ---------- Time ---------- */}
      <Section title="Time">
        <Time />
      </Section>

      {/* ---------- Divider ---------- */}
      <Section title="Divider(separator)">
        <div className="flex w-full flex-col gap-4">
          <Divider type="line-brown" />
          <Divider type="line-teal" />
          <Divider type="line-yellow" />
          <Divider type="wave-yellow" />
          <Divider type="dashed-brown" />
          <Divider type="dashed-teal" />
          <Divider type="dashed-yellow" />
        </div>
      </Section>

      {/* ---------- Footer(全幅) ---------- */}
      <div className="mt-6 flex w-full flex-col gap-3">
        <p className="text-center text-xs font-bold text-muted-foreground">Footer — tree / sea</p>
        <Footer type="tree" />
        <Footer type="sea" />
      </div>

      <Modal
        open={open}
        title="こんにちは 🌷"
        onClose={() => {
          setOpen(false);
        }}
        onOk={() => {
          setOpen(false);
        }}
      >
        どうぶつの森風のモーダルです。本文はタイプライターで表示されます。
      </Modal>
    </main>
  );
}
