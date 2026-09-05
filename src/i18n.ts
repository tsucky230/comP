// Locale helper for the extension's own runtime UI (notifications, QuickPick
// prompts, generated reports). comP has no vscode-nls/l10n pipeline, so this is
// a minimal stand-in: English is the default, Japanese is the only other
// variant, chosen from VS Code's own display language.
//
// package.json strings (command titles, the chat participant description) are
// localized separately through package.nls.json / package.nls.ja.json — VS
// Code's native mechanism for that file — since they can't run this code.

import * as vscode from "vscode";

/** True when VS Code's own display language is Japanese. */
export function isJapaneseLocale(): boolean {
  return vscode.env.language.toLowerCase().startsWith("ja");
}

/** Pick the string matching VS Code's display language; English is the default. */
export function t(en: string, ja: string): string {
  return isJapaneseLocale() ? ja : en;
}
