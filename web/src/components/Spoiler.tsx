import { useState, type KeyboardEvent, type MouseEvent, type ReactNode } from "react";
import { useI18n } from "@/i18n/react";

/*
 * A spoiler in Markdown (`||like this||`, found by `remarkSpoilers` in
 * lib/spoilers): covered until the reader clicks it or presses Enter on it,
 * then shown for as long as it's on screen. While covered, nothing inside
 * (links, mentions) can be clicked or selected. The cover fades away
 * (styles/app.css, `.spoiler`), at once with reduced motion.
 */
export function Spoiler({ children }: { children?: ReactNode; node?: unknown }) {
  const { t } = useI18n();
  const [shown, setShown] = useState(false);
  if (shown) return <span className="spoiler">{children}</span>;
  const reveal = (event: MouseEvent | KeyboardEvent) => {
    // A spoiler inside a link or a clickable row reveals, and does nothing else.
    event.preventDefault();
    event.stopPropagation();
    setShown(true);
  };
  return (
    <span
      className="spoiler"
      data-hidden=""
      role="button"
      tabIndex={0}
      aria-label={t("chat.spoiler.reveal")}
      onClick={reveal}
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") reveal(event);
      }}
    >
      <span aria-hidden>{children}</span>
    </span>
  );
}
