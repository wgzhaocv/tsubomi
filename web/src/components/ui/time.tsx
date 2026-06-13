import * as React from "react";

import { cn } from "@/lib/utils";

// animal-island-ui(guokaigdg)の Time を移植。原典は live clock:
// 1 秒ごとに new Date() で再描画し、曜日 / 月日 / 時:分(コロンは点滅)を表示。
// 色・寸法・3px 枠・グラデ面・font-weight は src/components/Time/time.module.less
// を厳密に踏襲。数字は font-weight 900。tick / フォーマットは原典そのまま。

export interface TimeProps {
  className?: string;
}

const weekdays = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

export function Time({ className }: TimeProps) {
  const [currentTime, setCurrentTime] = React.useState(new Date());

  React.useEffect(() => {
    // 原典どおり 1 秒間隔で現在時刻を更新(通常のブラウザ Date でよい)。
    const timer = setInterval(() => setCurrentTime(new Date()), 1000);
    return () => clearInterval(timer);
  }, []);

  const hours = currentTime.getHours().toString().padStart(2, "0");
  const minutes = currentTime.getMinutes().toString().padStart(2, "0");
  // <time dateTime> 用の機械可読値(ローカル時刻の HH:mm)。秒は表示しないので含めない。
  const isoTime = `${hours}:${minutes}`;

  return (
    // aria-live="off":毎秒更新 + コロン点滅で AT を埋め尽くさないよう明示的に黙らせる。
    <div
      aria-live="off"
      className={cn(
        // .acDatetime:クリーム縦グラデ + 3px 枠 + 角丸 18px、入場アニメ ac-fade-up。
        "inline-flex w-fit max-w-max items-center gap-5 self-start rounded-[18px] border-[3px] border-[#d4cfc3] bg-[linear-gradient(180deg,#fff_0%,#f8f8f0_100%)] px-9 py-4 animate-[animal-time-fade-up_0.5s_ease-out] max-md:gap-3 max-md:px-5 max-md:py-3",
        className,
      )}
    >
      {/* .acDate:右に 3px の薄茶セパレータ。曜日(緑)+ 月日(茶)を縦積み。 */}
      <div className="flex flex-col items-center border-r-[3px] border-[rgba(159,146,125,0.35)] pr-6 max-md:pr-3">
        <span className="text-[14px] font-black uppercase tracking-[1.5px] text-[#6fba2c] max-md:text-[11px]">
          {weekdays[currentTime.getDay()]}
        </span>
        <span className="text-[22px] font-extrabold text-[#8b7355] max-md:text-[16px]">
          {months[currentTime.getMonth()]} {currentTime.getDate()}
        </span>
      </div>
      {/* .acTime:48px / 茶 / 900。コロンは少し上げて 1s で点滅。
          機械可読な値は <time dateTime> が担うので、点滅コロンは装飾として扱う。 */}
      <time
        dateTime={isoTime}
        className="flex items-center text-[48px] font-black tracking-[2px] text-[#8b7355] max-md:text-[32px]"
      >
        {hours}
        {/* aria-hidden:点滅コロンは装飾。reduced-motion 時は motion-reduce で点滅を止める。 */}
        <span
          aria-hidden="true"
          className="relative top-[-0.08em] mx-px text-[48px] text-[#8b7355] animate-[animal-time-blink_1s_step-end_infinite] motion-reduce:animate-none max-md:text-[32px]"
        >
          :
        </span>
        {minutes}
      </time>
    </div>
  );
}

Time.displayName = "Time";
