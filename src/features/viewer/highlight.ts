/**
 * Syntax highlighting for the file viewer, with Shiki. Grammars load on demand; tokens carry both
 * a light and a dark color (as CSS variables) so the theme can switch without re-highlighting.
 */
import { useEffect, useState } from "react";
import { bundledLanguagesInfo } from "shiki/langs";
import type { BundledLanguage, BundledTheme, HighlighterGeneric, ThemedToken } from "shiki";

export type Tokens = ThemedToken[][];

/** Past these, files show plain: tokenizing them would stall the viewer. */
const MAX_CHARS = 400_000;
const MAX_LINES = 8_000;

const THEMES = { light: "github-light", dark: "github-dark" } as const;

/** Extensions and file names Shiki's ids and aliases don't cover. */
const EXTRA: Record<string, string> = {
  h: "c",
  hh: "cpp",
  hpp: "cpp",
  hxx: "cpp",
  cc: "cpp",
  cxx: "cpp",
  mts: "typescript",
  cts: "typescript",
  htm: "html",
  svg: "xml",
  plist: "xml",
  lock: "toml",
  conf: "ini",
  cfg: "ini",
  env: "dotenv",
  gitignore: "ignore",
  dockerfile: "docker",
  makefile: "make",
  gnumakefile: "make",
  cmakelists: "cmake",
  gemfile: "ruby",
  rakefile: "ruby",
  justfile: "just",
};

const byName = new Map<string, string>();
for (const info of bundledLanguagesInfo) {
  byName.set(info.id, info.id);
  for (const a of info.aliases ?? []) byName.set(a, info.id);
}

/** The Shiki language for `path`, from its extension or name, or null if none fits. */
export function languageFor(path: string): string | null {
  const name = (path.split(/[\\/]/).pop() ?? "").toLowerCase();
  const base = name.replace(/\.txt$/, "");
  const ext = base.includes(".") ? base.slice(base.lastIndexOf(".") + 1) : base.replace(/^\./, "");
  for (const key of [ext, name.split(".")[0], name.replace(/^\./, "")]) {
    const id = byName.get(EXTRA[key] ?? key);
    if (id) return id;
  }
  return null;
}

let highlighter: Promise<HighlighterGeneric<BundledLanguage, BundledTheme>> | null = null;
const getHighlighter = () =>
  (highlighter ??= import("shiki").then((s) => s.createHighlighter({
    themes: Object.values(THEMES), langs: [], engine: s.createJavaScriptRegexEngine({ forgiving: true }),
  })));

/** Tokenizes `code` as `lang`, or resolves null when it can't or shouldn't. */
export async function highlight(code: string, lang: string | null): Promise<Tokens | null> {
  if (!lang || code.length > MAX_CHARS) return null;
  const id = byName.get(lang.toLowerCase());
  if (!id) return null;
  try {
    const h = await getHighlighter();
    if (!h.getLoadedLanguages().includes(id)) await h.loadLanguage(id as BundledLanguage);
    const { tokens } = h.codeToTokens(code, { lang: id as BundledLanguage, themes: THEMES, defaultColor: false, tokenizeMaxLineLength: 2_000 });
    return tokens.length > MAX_LINES ? null : tokens;
  } catch {
    return null;
  }
}

/**
 * `code` tokenized as `lang`, null until ready (and when it can't be). While a changed `code`
 * re-highlights this keeps the last tokens, so callers check a line's tokens still match its text.
 */
export function useHighlight(code: string, lang: string | null): Tokens | null {
  const [result, setResult] = useState<{ lang: string | null; tokens: Tokens | null } | null>(null);
  useEffect(() => {
    let live = true;
    void highlight(code, lang).then((tokens) => live && setResult({ lang, tokens }));
    return () => {
      live = false;
    };
  }, [code, lang]);
  return result?.lang === lang ? result.tokens : null;
}

/** `line`'s tokens, if they're still for that text. */
export const lineTokens = (tokens: Tokens | null, i: number, line: string) => {
  const t = tokens?.[i];
  return t && t.map((x) => x.content).join("") === line ? t : null;
};

/** Inline style for one token: its colors per theme and its font style. */
export function tokenStyle(t: ThemedToken): React.CSSProperties {
  const style: Record<string, string> = { ...(t.htmlStyle as Record<string, string> | undefined) };
  const fs = t.fontStyle ?? 0;
  if (fs & 1) style.fontStyle = "italic";
  if (fs & 2) style.fontWeight = "600";
  if (fs & 4) style.textDecoration = "underline";
  return style as React.CSSProperties;
}
