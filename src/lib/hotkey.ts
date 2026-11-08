/**
 * Hotkey matching for in-window shortcuts.
 *
 * The config stores a human-readable spec ("Ctrl+D", "Cmd+Shift+K",
 * "Alt+Space") because that is what the user types and what the existing
 * global-shortcut setting already uses. Matching it against a DOM
 * `KeyboardEvent` here keeps the rules in one place and testable, instead of
 * string-comparing `e.key` in three components.
 *
 * Platform mapping:
 * - `Ctrl` → `ctrlKey` everywhere. On macOS the physical Control key also
 *   produces `ctrlKey`, which is what a user who typed "Ctrl" wants.
 * - `Cmd` / `Meta` / `Super` → `metaKey` everywhere, so a config synced from a
 *   Mac still works on Windows and vice versa.
 * - `Alt` / `Option` → `altKey`.
 */

export interface HotkeyEvent {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

export interface ParsedHotkey {
  key: string;
  ctrl: boolean;
  meta: boolean;
  alt: boolean;
  shift: boolean;
}

/** `"Ctrl+Shift+D"` → `{ key: "D", ctrl: true, shift: true, … }`. */
export function parseHotkey(spec: string): ParsedHotkey | null {
  const parts = spec
    .split("+")
    .map((part) => part.trim())
    .filter(Boolean);
  if (parts.length === 0) return null;

  const parsed: ParsedHotkey = {
    key: "",
    ctrl: false,
    meta: false,
    alt: false,
    shift: false,
  };

  for (const part of parts) {
    const lower = part.toLowerCase();
    if (lower === "ctrl" || lower === "control") parsed.ctrl = true;
    else if (lower === "cmd" || lower === "command" || lower === "meta" || lower === "super") {
      parsed.meta = true;
    } else if (lower === "alt" || lower === "option" || lower === "opt") parsed.alt = true;
    else if (lower === "shift") parsed.shift = true;
    else parsed.key = part;
  }

  // A spec with only modifiers ("Ctrl+") is not a shortcut.
  if (!parsed.key) return null;
  return parsed;
}

/**
 * True when this event is the given shortcut.
 *
 * Modifier state must match exactly: `Ctrl+D` must not fire on `Ctrl+Shift+D`,
 * otherwise a combination the user reserved for something else would trigger
 * the skill as a side effect.
 */
export function matchesHotkey(event: HotkeyEvent, spec: string): boolean {
  const parsed = parseHotkey(spec);
  if (!parsed) return false;
  if (event.ctrlKey !== parsed.ctrl) return false;
  if (event.metaKey !== parsed.meta) return false;
  if (event.altKey !== parsed.alt) return false;
  if (event.shiftKey !== parsed.shift) return false;

  const pressed = event.key;
  const wanted = parsed.key;
  if (pressed.length === 1 && wanted.length === 1) {
    return pressed.toLowerCase() === wanted.toLowerCase();
  }
  // Named keys: Space, Enter, ArrowUp, F5, … Case-insensitive, and accepting
  // both spellings of the two keys whose names differ between platforms and
  // browser generations ("Esc"/"Escape", " "/"Space").
  const normalise = (value: string) => {
    const lower = value.toLowerCase();
    if (lower === "esc") return "escape";
    if (lower === " " || lower === "spacebar") return "space";
    return lower;
  };
  return normalise(pressed) === normalise(wanted);
}

/** `"Ctrl+D"` → `"Ctrl + D"` for display in hints and toasts. */
export function hotkeyLabel(spec: string): string {
  const parsed = parseHotkey(spec);
  if (!parsed) return spec;
  const parts: string[] = [];
  if (parsed.ctrl) parts.push("Ctrl");
  if (parsed.meta) parts.push("Cmd");
  if (parsed.alt) parts.push("Alt");
  if (parsed.shift) parts.push("Shift");
  parts.push(parsed.key.length === 1 ? parsed.key.toUpperCase() : parsed.key);
  return parts.join(" + ");
}
