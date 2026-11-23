import React, { useEffect, useRef, useState } from "react";
import { X, Terminal, Search } from "lucide-react";
import { useTranslation } from "react-i18next";

/* Search capsule. One floating control, Spotlight-style: glyph, 22px query,
   result count, clear, settings. Solid surface,
   14px corners. A hairline runs along its base while a search is live. */

interface SearchBarProps {
  query: string;
  setQuery: (q: string) => void;
  inputRef: React.RefObject<HTMLInputElement | null>;
  onFocusInput: () => void;
  resultCount: number;
  /** Total matches before the display limit, when known. */
  total?: number;
  /** False when `total` is a lower bound, rendered as "1 234+". */
  totalExact?: boolean;
  /** Rows already in the list. Shown against the total: "показано 1 200 из 84 231". */
  shown?: number;
  /** True while rows are still being fetched in the background. */
  filling?: boolean;
  /** Pause/resume the background fill. Omitted when nothing is filling. */
  onToggleFill?: () => void;
  /** True while a listing waits for the crawl to finish before it can fill. */
  waitingForCrawl?: boolean;
  /** Icon buttons rendered at the right end of the capsule (settings). */
  trailing?: React.ReactNode;
}

export function SearchBar({
  query,
  setQuery,
  inputRef,
  onFocusInput,
  resultCount,
  total,
  totalExact = true,
  shown,
  filling,
  onToggleFill,
  waitingForCrawl,
  trailing,
}: SearchBarProps) {
  const [phIdx, setPhIdx] = useState(0);
  const rotateRef = useRef(false);

  const { t } = useTranslation("search");
  const { t: tc } = useTranslation("common");

  const placeholders = [
    t("placeholders.everything"),
    t("placeholders.invoice"),
    t("placeholders.pdf"),
    t("placeholders.chart"),
  ];

  useEffect(() => {
    rotateRef.current = query === "";
    if (!query) {
      const id = window.setInterval(() => {
        if (rotateRef.current) setPhIdx((i) => (i + 1) % placeholders.length);
      }, 5000);
      return () => window.clearInterval(id);
    }
  }, [query, placeholders.length]);

  const isCommandMode = query.startsWith(">") || query.startsWith("/");
  const count = total && total > resultCount ? total : resultCount;
  /* Ranked results fuse several tiers, so their union is a lower bound until
     every tier is fully enumerated. Say so instead of pretending the window is
     the whole answer. The "+" stays outside the translated string so i18next
     still sees a numeric `count` for plural selection. */
  const approximate = !totalExact && count > 0;
  /* How much of the match set is actually in the list.
   *
   * "1 234 результатов" alone says nothing about whether the rest is loaded:
   * the number stays the same while the grid is filled, so the only way to
   * check progress was to scroll to the bottom and look. Listing the rows and
   * the total side by side is the difference between "it is working, I can go
   * to bed" and "nothing happened overnight".
   */
  const shownCount = shown ?? resultCount;
  const showProgress = count > 0 && !isCommandMode && shownCount > 0;
  const progressLabel = showProgress
    ? t("loadedOf", { shown: shownCount.toLocaleString(), total: count.toLocaleString() })
    : "";
  const countLabel = count === 0 ? t("empty.noResults") : t("resultCount", { count });
  const percent =
    showProgress && count > 0 ? Math.min(100, Math.round((shownCount / count) * 100)) : 0;
  const Glyph = isCommandMode ? Terminal : Search;

  return (
    <div data-tauri-drag-region className="px-3.5 pb-1.5 pt-2.5">
      <div className="relative flex h-11 items-center gap-2.5 pl-3.5 pr-1.5">
        <Glyph className="h-5 w-5 shrink-0 text-muted" strokeWidth={1.75} />
        <input
          ref={inputRef}
          type="text"
          value={query}
          spellCheck={false}
          autoComplete="off"
          autoFocus
          onChange={(e) => setQuery(e.target.value)}
          placeholder={isCommandMode ? t("command") : placeholders[phIdx % placeholders.length]}
          onFocus={onFocusInput}
          aria-label={tc("actions.search")}
          className="min-w-0 flex-1 bg-transparent text-[19px] font-medium leading-none tracking-[-0.022em] text-fg outline-none placeholder:font-normal placeholder:text-subtle"
          style={{ caretColor: "var(--blue)" }}
        />

        {query && !isCommandMode && (
          <span className="t-meta fade-in flex shrink-0 items-center gap-2 text-subtle">
            {/* A hairline bar under the number: the fill moves it, so progress
                is visible from the top of the list without scrolling. */}
            {showProgress && shownCount < count && (
              <span
                className="relative block h-1 w-16 overflow-hidden rounded-full bg-surface-2"
                role="progressbar"
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={percent}
                aria-label={progressLabel}
              >
                <span
                  className="absolute inset-y-0 left-0 rounded-full bg-accent transition-[width] duration-300"
                  style={{ width: `${percent}%` }}
                />
              </span>
            )}
            <span className="tabular-nums">
              {showProgress ? progressLabel : `${countLabel}${approximate ? "+" : ""}`}
            </span>
            {waitingForCrawl && (
              <span
                className="flex items-center gap-1.5 text-[11px] text-muted"
                title={t("waitingForCrawlHint")}
              >
                <span className="dot-pulse h-1.5 w-1.5 rounded-full bg-warning" />
                {t("waitingForCrawl")}
              </span>
            )}
            {filling && onToggleFill && (
              <button
                type="button"
                onClick={onToggleFill}
                title={t("fillPauseHint")}
                className="rounded-full px-1.5 py-0.5 text-[11px] text-muted transition-colors hover:bg-surface-2 hover:text-fg"
              >
                {t("fillPause")}
              </button>
            )}
          </span>
        )}

        {query && (
          <button
            type="button"
            title={t("clearInput")}
            onClick={() => {
              setQuery("");
              inputRef.current?.focus();
            }}
            className="btn btn-ghost btn-icon h-9 w-9 shrink-0"
          >
            <X className="h-4 w-4" strokeWidth={1.75} />
          </button>
        )}

        {trailing && <div className="flex shrink-0 items-center">{trailing}</div>}
      </div>
    </div>
  );
}
