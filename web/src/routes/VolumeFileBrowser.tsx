import { useEffect, useRef, useState } from "react";
import {
  ChevronRight,
  Download,
  Eye,
  File as FileIcon,
  Folder,
  FolderPlus,
  Pencil,
  RefreshCw,
  Trash2,
  Upload,
} from "lucide-react";
import { useNavigate, useParams } from "react-router";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/modal";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import {
  type FileEntry,
  downloadFile,
  formatBytes,
  previewUrl,
  useDeleteEntry,
  useListDir,
  useMkdir,
  useMove,
  useUpload,
} from "@/lib/volumes";

// 假根のファイルブラウザ。**現在のディレクトリは URL がそのまま持つ**
// (/volumes/:id/files/path/to/dir → /path/to/dir)。React Router の splat を読み、
// リンク生成時は各セグメントを encodeURIComponent する(特殊文字・日本語名対応)。
// パストラバーサル防御はサーバ(safe_path)が担う。

function joinPath(base: string, name: string): string {
  return base ? `${base}/${name}` : name;
}

// 假根相対パス → ブラウザ URL(各セグメントをエンコード)。
function browseUrl(id: string, path: string): string {
  const enc = path.split("/").filter(Boolean).map(encodeURIComponent).join("/");
  return enc ? `/volumes/${id}/files/${enc}` : `/volumes/${id}/files`;
}

export default function VolumeFileBrowser() {
  const params = useParams();
  const id = params.id ?? "";
  // splat("*")= 假根からの現在パス。前後スラッシュを正規化(ルートは "")。
  const currentPath = (params["*"] ?? "").replace(/^\/+|\/+$/g, "");
  const navigate = useNavigate();

  const { data, isPending, error, refetch, isFetching } = useListDir(id, currentPath);
  const upload = useUpload(id);
  const mkdir = useMkdir(id);
  const del = useDeleteEntry(id);
  const move = useMove(id);

  // 一覧/各操作のうち最初のエラーをまとめて表示する。
  const anyError = error || upload.error || mkdir.error || del.error || move.error;

  const fileInputRef = useRef<HTMLInputElement>(null);
  // ドラッグ&ドロップの深さカウンタ(子要素を跨ぐ dragenter/leave のちらつき防止)。
  const dragDepth = useRef(0);

  // アップロード進捗(0–100)。null = 非アップロード中。複数ファイルは簡易集計
  // (各 onProgress で上書き表示、全部 settle で消す)。
  const [uploadPct, setUploadPct] = useState<number | null>(null);
  const [dragActive, setDragActive] = useState(false);
  const [mkdirOpen, setMkdirOpen] = useState(false);
  const [mkdirName, setMkdirName] = useState("");
  const [renameTarget, setRenameTarget] = useState<FileEntry | null>(null);
  const [renameName, setRenameName] = useState("");
  const [deleteTarget, setDeleteTarget] = useState<FileEntry | null>(null);

  const segments = currentPath ? currentPath.split("/") : [];

  // 複数ファイルを現在ディレクトリへ並行アップロード(ファイル選択 / ドロップ共用)。
  const uploadFiles = (files: File[]) => {
    if (files.length === 0) return;
    setUploadPct(0);
    let remaining = files.length;
    for (const file of files) {
      upload.mutate(
        { path: joinPath(currentPath, file.name), file, onProgress: setUploadPct },
        {
          onSettled: () => {
            remaining -= 1;
            if (remaining === 0) setUploadPct(null);
          },
        },
      );
    }
  };

  const onPickFiles = (e: React.ChangeEvent<HTMLInputElement>) => {
    const files = Array.from(e.currentTarget.files ?? []);
    e.currentTarget.value = ""; // 同じファイルを連続で選べるよう値をリセット
    uploadFiles(files);
  };

  // window リスナは一度だけ張る。最新の uploadFiles は ref 経由で参照(stale closure 回避)。
  const uploadFilesRef = useRef(uploadFiles);
  useEffect(() => {
    uploadFilesRef.current = uploadFiles;
  });

  // ドロップゾーンは「左メニューを除く画面全体」。window でドラッグを拾い、ブラウザ既定の
  // 「ファイルを開く」遷移を全画面で抑止する。md+(≥768px)の 256px サイドバー上への
  // ドロップは無視し、content 領域だけ受け付ける(深さカウンタで子要素の出入りを吸収)。
  useEffect(() => {
    const hasFiles = (e: DragEvent) => Array.from(e.dataTransfer?.types ?? []).includes("Files");
    const onEnter = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      dragDepth.current += 1;
      setDragActive(true);
    };
    const onOver = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault(); // drop 受け付け + 既定遷移の抑止
      if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
    };
    const onLeave = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      dragDepth.current -= 1;
      if (dragDepth.current <= 0) {
        dragDepth.current = 0;
        setDragActive(false);
      }
    };
    const onDropWin = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      dragDepth.current = 0;
      setDragActive(false);
      // 左メニュー(md+ の 256px サイドバー)上へのドロップは無視。
      if (window.innerWidth >= 768 && e.clientX < 256) return;
      uploadFilesRef.current(Array.from(e.dataTransfer?.files ?? []));
    };
    window.addEventListener("dragenter", onEnter);
    window.addEventListener("dragover", onOver);
    window.addEventListener("dragleave", onLeave);
    window.addEventListener("drop", onDropWin);
    return () => {
      window.removeEventListener("dragenter", onEnter);
      window.removeEventListener("dragover", onOver);
      window.removeEventListener("dragleave", onLeave);
      window.removeEventListener("drop", onDropWin);
    };
  }, []);

  const submitMkdir = () => {
    const trimmed = mkdirName.trim();
    if (!trimmed) return;
    mkdir.mutate(joinPath(currentPath, trimmed), {
      onSuccess: () => {
        setMkdirOpen(false);
        setMkdirName("");
      },
    });
  };

  const submitRename = () => {
    const trimmed = renameName.trim();
    if (!trimmed || !renameTarget) return;
    move.mutate(
      { from: joinPath(currentPath, renameTarget.name), to: joinPath(currentPath, trimmed) },
      { onSuccess: () => setRenameTarget(null) },
    );
  };

  return (
    <div className="flex flex-col gap-4">
      {/* 全内容領域(左メニューを除く)を覆うドロップオーバーレイ。fixed + md:left-64。
          pointer-events-none で window のドラッグ判定を邪魔しない。 */}
      {dragActive && (
        <div className="pointer-events-none fixed inset-0 z-50 grid place-items-center bg-[rgba(12,192,181,0.1)] md:left-64">
          <div className="flex flex-col items-center gap-2 rounded-2xl border-2 border-dashed border-[#0CC0B5] bg-card px-8 py-6 shadow-[0_4px_16px_rgba(61,52,40,0.15)]">
            <Upload className="size-8 text-[#0CC0B5]" />
            <p className="text-base font-bold text-foreground">ここにドロップしてアップロード</p>
            <p className="text-xs font-medium text-muted-foreground">
              {currentPath ? `/${currentPath}` : "ルート"} に保存(複数可)
            </p>
          </div>
        </div>
      )}

      {/* パンくず(URL のパスをそのまま表示)+ ツールバー。 */}
      <div className="flex flex-wrap items-center justify-between gap-3">
        <nav
          aria-label="現在のパス"
          className="flex flex-wrap items-center gap-1 text-sm font-semibold text-muted-foreground"
        >
          <button
            type="button"
            onClick={() => navigate(browseUrl(id, ""))}
            className="rounded-lg px-1.5 py-0.5 text-foreground outline-none hover:text-[#11a89b] focus-visible:[outline:2px_solid_#19c8b9]"
          >
            /
          </button>
          {segments.map((seg, i) => {
            const target = segments.slice(0, i + 1).join("/");
            const last = i === segments.length - 1;
            return (
              <span key={target} className="flex items-center gap-1">
                <ChevronRight className="size-3.5 text-muted-foreground/60" />
                {last ? (
                  <span className="px-1 py-0.5 font-bold text-foreground">{seg}</span>
                ) : (
                  <button
                    type="button"
                    onClick={() => navigate(browseUrl(id, target))}
                    className="rounded-lg px-1 py-0.5 outline-none hover:text-[#11a89b] focus-visible:[outline:2px_solid_#19c8b9]"
                  >
                    {seg}
                  </button>
                )}
              </span>
            );
          })}
        </nav>

        <div className="flex flex-wrap gap-2">
          <input ref={fileInputRef} type="file" multiple hidden onChange={onPickFiles} />
          <Button
            type="default"
            size="small"
            icon={<RefreshCw className={isFetching ? "size-4 animate-spin" : "size-4"} />}
            onClick={() => {
              refetch();
            }}
          >
            更新
          </Button>
          <Button
            type="default"
            size="small"
            icon={<FolderPlus className="size-4" />}
            onClick={() => {
              setMkdirName("");
              setMkdirOpen(true);
            }}
          >
            新規フォルダ
          </Button>
          <Button
            type="primary"
            size="small"
            loading={upload.isPending}
            icon={<Upload className="size-4" />}
            onClick={() => fileInputRef.current?.click()}
          >
            アップロード
          </Button>
        </div>
      </div>

      {/* アップロード進捗バー(XHR の upload.onprogress 由来)。 */}
      {uploadPct !== null && (
        <div className="flex items-center gap-2">
          <div className="h-2 flex-1 overflow-hidden rounded-full bg-[rgba(196,184,158,0.3)]">
            <div
              className="h-full rounded-full bg-[#0CC0B5] transition-[width] duration-150 ease-out"
              style={{ width: `${uploadPct}%` }}
            />
          </div>
          <span className="text-xs font-bold tabular-nums text-muted-foreground">{uploadPct}%</span>
        </div>
      )}

      {anyError && <p className="text-sm font-semibold text-[#e05a5a]">{anyError.message}</p>}

      {/* 一覧テーブル。フォルダ行は行全体クリックで遷移、ファイル行は名前非クリック
          (操作は右のアイコンボタン)。ボタンは stopPropagation で行クリックへ伝播しない。 */}
      <div className="overflow-hidden rounded-2xl border-2 border-[#c4b89e] bg-card">
        <div className="max-h-[62vh] overflow-auto">
          <table className="w-full border-collapse text-sm">
            <thead>
              <tr>
                {["名前", "サイズ", "更新日時", ""].map((h, i) => (
                  <th
                    key={h || i}
                    className="sticky top-0 z-10 border-b-2 border-[#c4b89e] bg-[#e8e1cc] px-3 py-2 text-left font-bold whitespace-nowrap text-[#794f27]"
                  >
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {isPending ? (
                <Row colSpan={4}>読み込み中…</Row>
              ) : !data || data.entries.length === 0 ? (
                <Row colSpan={4}>(空のディレクトリ)</Row>
              ) : (
                data.entries.map((e) => {
                  const path = joinPath(currentPath, e.name);
                  const enterFolder = () => {
                    navigate(browseUrl(id, path));
                  };
                  return (
                    <tr
                      key={e.name}
                      className={cn(
                        "even:bg-[rgba(196,184,158,0.12)]",
                        // フォルダ行は行全体をクリック可能に(名前だけだと当たり判定が狭い)。
                        e.is_dir &&
                          "cursor-pointer outline-none hover:bg-[rgba(25,200,185,0.08)] focus-visible:[outline:2px_solid_#19c8b9] focus-visible:[outline-offset:-2px]",
                      )}
                      {...(e.is_dir
                        ? {
                            role: "button",
                            tabIndex: 0,
                            "aria-label": `フォルダ ${e.name} を開く`,
                            onClick: enterFolder,
                            onKeyDown: (ev: React.KeyboardEvent) => {
                              if (ev.key === "Enter" || ev.key === " ") {
                                ev.preventDefault();
                                enterFolder();
                              }
                            },
                          }
                        : {})}
                    >
                      <td className="px-3 py-2">
                        {/* 名前はテキスト表示のみ(フォルダ遷移は行全体、ファイル操作はボタン)。 */}
                        <span className="flex items-center gap-2 font-semibold text-foreground">
                          {e.is_dir ? (
                            <Folder className="size-4 shrink-0 text-[#dba90e]" />
                          ) : (
                            <FileIcon className="size-4 shrink-0 text-muted-foreground" />
                          )}
                          <span className="min-w-0 truncate">
                            {e.is_dir ? `${e.name}/` : e.name}
                          </span>
                        </span>
                      </td>
                      <td className="px-3 py-2 whitespace-nowrap tabular-nums text-muted-foreground">
                        {e.is_dir ? "—" : formatBytes(e.size)}
                      </td>
                      <td className="px-3 py-2 whitespace-nowrap text-muted-foreground">
                        {e.modified ? new Date(e.modified).toLocaleString("ja-JP") : "—"}
                      </td>
                      <td className="px-3 py-2">
                        <div className="flex justify-end gap-1">
                          {!e.is_dir && (
                            <>
                              <IconButton
                                label="プレビュー(新しいタブで開く)"
                                onClick={() =>
                                  window.open(previewUrl(id, path), "_blank", "noopener,noreferrer")
                                }
                              >
                                <Eye className="size-4" />
                              </IconButton>
                              <IconButton
                                label="ダウンロード"
                                onClick={() => downloadFile(id, path)}
                              >
                                <Download className="size-4" />
                              </IconButton>
                            </>
                          )}
                          <IconButton
                            label="名前を変更 / 移動"
                            onClick={() => {
                              setRenameTarget(e);
                              setRenameName(e.name);
                            }}
                          >
                            <Pencil className="size-4" />
                          </IconButton>
                          <IconButton label="削除" danger onClick={() => setDeleteTarget(e)}>
                            <Trash2 className="size-4" />
                          </IconButton>
                        </div>
                      </td>
                    </tr>
                  );
                })
              )}
            </tbody>
          </table>
        </div>
      </div>

      {/* 新規フォルダ */}
      <Modal
        open={mkdirOpen}
        title="新規フォルダ"
        typewriter={false}
        width={460}
        onClose={() => setMkdirOpen(false)}
        footer={
          <>
            <Button type="text" onClick={() => setMkdirOpen(false)}>
              キャンセル
            </Button>
            <Button
              type="primary"
              loading={mkdir.isPending}
              disabled={!mkdirName.trim()}
              onClick={submitMkdir}
            >
              作成
            </Button>
          </>
        }
      >
        <div className="flex w-full flex-col gap-3">
          <Input
            label="フォルダ名"
            placeholder="例:images"
            value={mkdirName}
            autoFocus
            onChange={(e) => setMkdirName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") submitMkdir();
            }}
            description={`${currentPath ? `/${currentPath}` : "ルート"} の中に作成します。`}
          />
        </div>
      </Modal>

      {/* 名前変更 / 移動 */}
      <Modal
        open={renameTarget !== null}
        title="名前を変更 / 移動"
        typewriter={false}
        width={460}
        onClose={() => setRenameTarget(null)}
        footer={
          <>
            <Button type="text" onClick={() => setRenameTarget(null)}>
              キャンセル
            </Button>
            <Button
              type="primary"
              loading={move.isPending}
              disabled={!renameName.trim()}
              onClick={submitRename}
            >
              変更
            </Button>
          </>
        }
      >
        <div className="flex w-full flex-col gap-3">
          <Input
            label="新しい名前(現在のフォルダからの相対パス)"
            value={renameName}
            autoFocus
            onChange={(e) => setRenameName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") submitRename();
            }}
            description="サブフォルダへ移動するには path/to/name のように指定します。"
          />
        </div>
      </Modal>

      {/* 削除確認 */}
      <Modal
        open={deleteTarget !== null}
        title="削除"
        typewriter={false}
        width={460}
        onClose={() => setDeleteTarget(null)}
        footer={
          <>
            <Button type="text" onClick={() => setDeleteTarget(null)}>
              キャンセル
            </Button>
            <Button
              type="primary"
              danger
              loading={del.isPending}
              onClick={() => {
                if (!deleteTarget) return;
                del.mutate(joinPath(currentPath, deleteTarget.name), {
                  onSuccess: () => setDeleteTarget(null),
                });
              }}
            >
              削除する
            </Button>
          </>
        }
      >
        <p>
          <strong>{deleteTarget?.name}</strong> を削除しますか?
          {deleteTarget?.is_dir && "(フォルダの中身ごと削除されます)"}
        </p>
      </Modal>
    </div>
  );
}

// テーブルの全幅メッセージ行(読み込み中 / 空)。
function Row({ colSpan, children }: { colSpan: number; children: React.ReactNode }) {
  return (
    <tr>
      <td
        colSpan={colSpan}
        className="px-3 py-6 text-center text-sm font-medium text-muted-foreground"
      >
        {children}
      </td>
    </tr>
  );
}

// 行内のアイコンボタン(プレビュー / ダウンロード / リネーム / 削除)。アクセシブル名は
// aria-label、ツールチップは Tooltip コンポーネント(上に表示、hover/focus 触発)。
// クリックは行(フォルダ遷移)へ伝播させない。
function IconButton({
  label,
  danger,
  onClick,
  children,
}: {
  label: string;
  danger?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <Tooltip title={label} placement="top">
      <button
        type="button"
        aria-label={label}
        onClick={(e) => {
          e.stopPropagation();
          onClick();
        }}
        className={cn(
          "grid size-8 place-items-center rounded-lg text-muted-foreground outline-none focus-visible:[outline:2px_solid_#19c8b9]",
          danger
            ? "hover:bg-[rgba(201,68,68,0.1)] hover:text-[#c94444]"
            : "hover:bg-[rgba(25,200,185,0.12)] hover:text-[#11a89b]",
        )}
      >
        {children}
      </button>
    </Tooltip>
  );
}
