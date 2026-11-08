import { describe, expect, it } from "vitest";
import { hotkeyLabel, matchesHotkey, parseHotkey } from "./hotkey";

const event = (over: Partial<Parameters<typeof matchesHotkey>[0]>) => ({
  key: "",
  ctrlKey: false,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  ...over,
});

describe("parseHotkey", () => {
  it("reads modifiers and the key", () => {
    expect(parseHotkey("Ctrl+Shift+D")).toEqual({
      key: "D",
      ctrl: true,
      meta: false,
      alt: false,
      shift: true,
    });
  });

  it("accepts the aliases people actually type", () => {
    expect(parseHotkey("Cmd+D")?.meta).toBe(true);
    expect(parseHotkey("Command+D")?.meta).toBe(true);
    expect(parseHotkey("Super+D")?.meta).toBe(true);
    expect(parseHotkey("Option+K")?.alt).toBe(true);
    expect(parseHotkey("Control+K")?.ctrl).toBe(true);
  });

  it("rejects modifier-only specs", () => {
    expect(parseHotkey("Ctrl+")).toBeNull();
    expect(parseHotkey("")).toBeNull();
  });
});

describe("matchesHotkey", () => {
  it("matches a plain Ctrl+D press", () => {
    expect(matchesHotkey(event({ key: "d", ctrlKey: true }), "Ctrl+D")).toBe(true);
  });

  it("is case-insensitive for letters", () => {
    expect(matchesHotkey(event({ key: "D", ctrlKey: true }), "Ctrl+d")).toBe(true);
  });

  it("refuses extra or missing modifiers", () => {
    expect(matchesHotkey(event({ key: "d", ctrlKey: true, shiftKey: true }), "Ctrl+D")).toBe(false);
    expect(matchesHotkey(event({ key: "d" }), "Ctrl+D")).toBe(false);
    expect(matchesHotkey(event({ key: "d", ctrlKey: true, altKey: true }), "Ctrl+D")).toBe(false);
  });

  it("maps Cmd to the meta key", () => {
    expect(matchesHotkey(event({ key: "d", metaKey: true }), "Cmd+D")).toBe(true);
    expect(matchesHotkey(event({ key: "d", ctrlKey: true }), "Cmd+D")).toBe(false);
  });

  it("handles named keys and the Esc spelling", () => {
    expect(matchesHotkey(event({ key: " " }), "Space")).toBe(true);
    expect(matchesHotkey(event({ key: "Space" }), "Space")).toBe(true);
    expect(matchesHotkey(event({ key: "Escape" }), "Esc")).toBe(true);
    expect(matchesHotkey(event({ key: "ArrowUp", altKey: true }), "Alt+ArrowUp")).toBe(true);
  });
});

describe("hotkeyLabel", () => {
  it("spaces out a spec for display", () => {
    expect(hotkeyLabel("Ctrl+D")).toBe("Ctrl + D");
    expect(hotkeyLabel("Cmd+Shift+K")).toBe("Cmd + Shift + K");
    expect(hotkeyLabel("Alt+Space")).toBe("Alt + Space");
  });

  it("returns the raw spec when it cannot be parsed", () => {
    expect(hotkeyLabel("Ctrl+")).toBe("Ctrl+");
  });
});
