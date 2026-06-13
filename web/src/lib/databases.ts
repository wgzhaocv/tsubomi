import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

// database リソースのサーバ状態。lib/auth.ts と同じ作法:生の fetch + それを包む
// TanStack Query フック。一覧は Query が単一の真実源(props で配らない)。
// 接続文字列(= パスワード)は秘密なので Query にキャッシュせず、表示要求時に
// mutation で都度取得して画面ローカルに置く。

export type Database = {
  id: string;
  display_name: string;
  anon_seq: number;
  created_at: string;
  rotated_at: string | null;
};

// 1 文ぶんの結果。SELECT 系は columns/rows、それ以外は columns 空 + rows_affected。
export type ResultSet = {
  columns: string[];
  rows: (string | null)[][];
  row_count: number;
  truncated: boolean;
  rows_affected: number;
};

// /query の応答。複数文を投げると文ごとに 1 集合ずつ(混ざらない)。
export type QueryResponse = {
  results: ResultSet[];
};

// 空の結果集合(テーブル系フックで results が空のときのフォールバック)。
const EMPTY_SET: ResultSet = {
  columns: [],
  rows: [],
  row_count: 0,
  truncated: false,
  rows_affected: 0,
};

export const dbKeys = {
  all: ["databases"] as const,
  detail: (id: string) => ["databases", id] as const,
};

// エラー本文(サーバは AppError の日本語メッセージを text で返す)を投げる。
async function failBody(res: Response): Promise<never> {
  const body = await res.text().catch(() => "");
  throw new Error(body || `HTTP ${res.status}`);
}

async function fetchDatabases(): Promise<Database[]> {
  const res = await fetch("/api/databases");
  if (!res.ok) return failBody(res);
  return (await res.json()) as Database[];
}

export function useDatabases() {
  return useQuery({ queryKey: dbKeys.all, queryFn: fetchDatabases });
}

export function useCreateDatabase() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (name: string): Promise<Database> => {
      const res = await fetch("/api/databases", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name }),
      });
      if (!res.ok) return failBody(res);
      return (await res.json()) as Database;
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: dbKeys.all }),
  });
}

export function useDeleteDatabase() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (id: string): Promise<void> => {
      const res = await fetch(`/api/databases/${id}`, { method: "DELETE" });
      if (!res.ok) return failBody(res);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: dbKeys.all }),
  });
}

// 接続文字列を都度取得する(秘密なのでキャッシュしない)。表示要求時のみ呼ぶ。
export function useRevealUrl() {
  return useMutation({
    mutationFn: async (id: string): Promise<string> => {
      const res = await fetch(`/api/databases/${id}/url`);
      if (!res.ok) return failBody(res);
      return ((await res.json()) as { url: string }).url;
    },
  });
}

// rotate:human のパスワードを差し替え、新しい接続文字列を返す。
// rotated_at が変わるので一覧/詳細を無効化する。
export function useRotate() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (id: string): Promise<string> => {
      const res = await fetch(`/api/databases/${id}/rotate`, { method: "POST" });
      if (!res.ok) return failBody(res);
      return ((await res.json()) as { url: string }).url;
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: dbKeys.all }),
  });
}

// その DB 自身の human 資格でサーバ側が任意 SQL を実行する低レベル関数。
// SQL コンソール(mutation)とテーブル閲覧(query)の共通入口。
async function runSql(id: string, sql: string): Promise<QueryResponse> {
  const res = await fetch(`/api/databases/${id}/query`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ sql }),
  });
  if (!res.ok) return failBody(res);
  return (await res.json()) as QueryResponse;
}

// テーブル系フックは単文(1 集合)なので先頭の結果集合だけ取り出す。
async function runSingle(id: string, sql: string): Promise<ResultSet> {
  const r = await runSql(id, sql);
  return r.results[0] ?? EMPTY_SET;
}

// web SQL クライアント。その DB 自身の human 資格でサーバ側が実行する。
export function useRunQuery(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (sql: string) => runSql(id, sql),
    // 任意 SQL は CREATE / DROP / ALTER やデータ変更を含みうるので、その DB の
    // テーブル系クエリ(一覧 / 構造 / 行)を失効させる。テーブル画面へ移ると取り直す
    // (例:SQL でテーブルを作ってからテーブル画面へ行くと自動で現れる)。
    onSuccess: () => qc.invalidateQueries({ queryKey: dbKeys.detail(id) }),
  });
}

// ===== テーブル閲覧(専用 API は持たず、/query へ information_schema /
// SELECT * を投げて組み立てる。実行は当該 DB 自身の資格 = 権限昇格は起きない)。=====

// 識別子を二重引用符で安全に包む(内部の " は "" にエスケープ)。
function quoteIdent(name: string): string {
  return `"${name.replace(/"/g, '""')}"`;
}
// 文字列リテラルを単引用符で安全に包む(内部の ' は '' にエスケープ)。
function quoteLiteral(value: string): string {
  return `'${value.replace(/'/g, "''")}'`;
}

export type TableColumn = {
  name: string;
  type: string;
  nullable: boolean;
  default: string | null;
};

// public スキーマの BASE TABLE 一覧。
export function useTables(id: string) {
  return useQuery({
    queryKey: [...dbKeys.detail(id), "tables"],
    queryFn: async (): Promise<string[]> => {
      const r = await runSingle(
        id,
        "SELECT table_name FROM information_schema.tables " +
          "WHERE table_schema = 'public' AND table_type = 'BASE TABLE' " +
          "ORDER BY table_name",
      );
      return r.rows.map((row) => row[0]).filter((n): n is string => !!n);
    },
  });
}

// テーブルの列定義(STRUCTURE タブ)。
export function useTableColumns(id: string, table: string | undefined) {
  return useQuery({
    queryKey: [...dbKeys.detail(id), "columns", table],
    enabled: !!table,
    queryFn: async (): Promise<TableColumn[]> => {
      const r = await runSingle(
        id,
        "SELECT column_name, data_type, is_nullable, column_default " +
          "FROM information_schema.columns " +
          `WHERE table_schema = 'public' AND table_name = ${quoteLiteral(table!)} ` +
          "ORDER BY ordinal_position",
      );
      return r.rows.map((row) => ({
        name: row[0] ?? "",
        type: row[1] ?? "",
        nullable: row[2] === "YES",
        default: row[3],
      }));
    },
  });
}

// テーブルの行データ(DATA タブ、先頭 100 行)。
export function useTableRows(id: string, table: string | undefined) {
  return useQuery({
    queryKey: [...dbKeys.detail(id), "rows", table],
    enabled: !!table,
    queryFn: () => runSingle(id, `SELECT * FROM "public".${quoteIdent(table!)} LIMIT 100`),
  });
}
