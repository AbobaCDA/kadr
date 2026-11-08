import { useEffect } from "react";
import { CheckCircle2, FolderOpen, TriangleAlert, X } from "lucide-react";

/**
 * One-line feedback for actions that move files around.
 *
 * Deliberately not a browser-style toast stack: the collect skill has exactly
 * one thing to report ("filed here", or why it did not), and it offers the two
 * follow-ups a user actually wants — open the folder, or point it somewhere
 * else. Auto-dismisses, but stays put while the pointer is over it.
 */

export interface ToastAction {
  label: string;
  onClick: () => void;
}

export interface ToastMessage {
  kind: "ok" | "error";
  text: string;
  actions?: ToastAction[];
}

export function Toast({
  message,
  onDismiss,
  autoHideMs = 7000,
}: {
  message: ToastMessage;
  onDismiss: () => void;
  autoHideMs?: number;
}) {
  useEffect(() => {
    const id = window.setTimeout(onDismiss, autoHideMs);
    return () => window.clearTimeout(id);
  }, [message, onDismiss, autoHideMs]);

  const Icon = message.kind === "ok" ? CheckCircle2 : TriangleAlert;

  return (
    <div
      role="status"
      aria-live="polite"
      className="overlay-in pointer-events-auto fixed inset-x-0 bottom-3 z-[80] mx-auto flex w-fit max-w-[min(92vw,560px)] items-center gap-3 rounded-control border border-line bg-elevated px-3 py-2 shadow-[var(--shadow-float)]"
    >
      <Icon
        className={`h-4 w-4 shrink-0 ${message.kind === "ok" ? "text-accent" : "text-danger"}`}
        strokeWidth={1.75}
      />
      <span className="min-w-0 truncate text-[12.5px] text-fg" title={message.text}>
        {message.text}
      </span>
      {message.actions?.map((action) => (
        <button
          key={action.label}
          type="button"
          onClick={action.onClick}
          className="shrink-0 text-[12px] text-muted underline decoration-line-strong underline-offset-4 transition-colors hover:text-fg"
        >
          {action.label}
        </button>
      ))}
      <button
        type="button"
        onClick={onDismiss}
        aria-label="Dismiss"
        className="shrink-0 text-subtle transition-colors hover:text-fg"
      >
        <X className="h-3.5 w-3.5" />
      </button>
    </div>
  );
}

/** Folder icon re-exported so callers do not import lucide twice for one action. */
export const FolderActionIcon = FolderOpen;
