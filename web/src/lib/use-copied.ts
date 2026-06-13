import { useCallback, useEffect, useRef, useState } from "react";

// 「コピー」→ 一定時間後に表示が戻る、コピー成功フラグの共通フック。
// クリップボード書き込み + リセットタイマーの後始末をまとめる(複数のコピーボタンで共用)。
export function useCopied(resetMs = 1500) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  // アンマウント時にタイマーを止める(戻ってこない setState を避ける)。
  useEffect(() => () => clearTimeout(timer.current), []);

  const copy = useCallback(
    (text: string) => {
      void navigator.clipboard.writeText(text).then(() => {
        setCopied(true);
        clearTimeout(timer.current);
        timer.current = setTimeout(() => setCopied(false), resetMs);
      });
    },
    [resetMs],
  );

  return { copied, copy };
}
