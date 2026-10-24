import { memo, useEffect, useState } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { Check, Copy } from "lucide-react";
import { Linkify, linkifyChildren, MarkdownLink } from "./Links";

let highlighter: Promise<typeof import("shiki")> | undefined;

function CodeBlock({
  code,
  lang,
  dark,
}: {
  code: string;
  lang: string;
  dark: boolean;
}) {
  const [html, setHtml] = useState<string>();
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    let live = true;
    highlighter ??= import("shiki");
    highlighter
      .then((s) =>
        s.codeToHtml(code, {
          lang: lang || "text",
          theme: dark ? "github-dark-default" : "github-light-default",
        }),
      )
      .then((h) => live && setHtml(h))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [code, lang, dark]);
  return (
    <div className="cw-code">
      <div className="cw-code-head">
        <span>{lang || "text"}</span>
        <button
          className="cw-icon-btn"
          onClick={() => {
            void navigator.clipboard.writeText(code);
            setCopied(true);
            setTimeout(() => setCopied(false), 1200);
          }}
          title="Copy"
        >
          {copied ? <Check size={13} /> : <Copy size={13} />}
        </button>
      </div>
      {html ? (
        <div
          className="cw-code-body"
          dangerouslySetInnerHTML={{ __html: html }}
        />
      ) : (
        <pre className="cw-code-body">
          <code>{code}</code>
        </pre>
      )}
    </div>
  );
}

const remarkPlugins = [remarkGfm];

// Built once per theme: a fresh `components` object each render would hand
// react-markdown new component types, remounting every code block on each
// streamed token (losing its highlight and making the transcript jump).
function makeComponents(dark: boolean): Components {
  return {
    code({ className, children }) {
      const m = /language-(\w+)/.exec(className ?? "");
      const src = String(children ?? "");
      if (!m && !src.includes("\n"))
        return (
          <code className="cw-inline">
            <Linkify text={src} />
          </code>
        );
      return (
        <CodeBlock
          code={src.replace(/\n$/, "")}
          lang={m?.[1] ?? ""}
          dark={dark}
        />
      );
    },
    pre({ children }) {
      return <>{children}</>;
    },
    a: ({ href, children }) => (
      <MarkdownLink href={href}>{children}</MarkdownLink>
    ),
    p: ({ children }) => <p>{linkifyChildren(children)}</p>,
    li: ({ children }) => <li>{linkifyChildren(children)}</li>,
    td: ({ children }) => <td>{linkifyChildren(children)}</td>,
    th: ({ children }) => <th>{linkifyChildren(children)}</th>,
  };
}
const componentsLight = makeComponents(false);
const componentsDark = makeComponents(true);

export const Markdown = memo(function Markdown({
  text,
  dark,
}: {
  text: string;
  dark: boolean;
}) {
  return (
    <div className="cw-md">
      <ReactMarkdown
        remarkPlugins={remarkPlugins}
        components={dark ? componentsDark : componentsLight}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
});
