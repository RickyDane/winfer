import { createContext, memo, useContext, useEffect, useMemo, useRef, useState } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { Check, Copy } from "lucide-react";
import { Linkify, linkifyChildren, MarkdownLink } from "./Links";
import { splitBlocks } from "./mdBlocks";
import { highlightCode } from "./highlightCode";

/** Whether the markdown block being rendered is still streaming in. */
const StreamingContext = createContext(false);

function CodeBlock({
  code,
  lang,
  dark,
}: {
  code: string;
  lang: string;
  dark: boolean;
}) {
  const streaming = useContext(StreamingContext);
  const root = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(false);
  const [result, setResult] = useState<{ code: string; lang: string; dark: boolean; html: string | null }>();
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    const el = root.current;
    if (!el) return;
    if (typeof IntersectionObserver === "undefined") {
      setVisible(true);
      return;
    }
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        setVisible(true);
        observer.disconnect();
      }
    }, { rootMargin: "300px" });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    // Highlighting a block that's still streaming would redo the whole block on every token
    // (tens of milliseconds for a few KB, on the main thread): it waits until it's complete.
    if (streaming || !visible) return;
    let live = true;
    void highlightCode(code, lang, dark).then((html) => live && setResult({ code, lang, dark, html }));
    return () => {
      live = false;
    };
  }, [code, lang, dark, streaming, visible]);
  const html = result?.code === code && result.lang === lang && result.dark === dark ? result.html : null;
  return (
    <div className="cw-code" ref={root}>
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
      {html && !streaming ? (
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
      const m = /language-([^\s]+)/.exec(className ?? "");
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

const Block = memo(function Block({
  text,
  components,
  streaming,
}: {
  text: string;
  components: Components;
  streaming: boolean;
}) {
  return (
    <StreamingContext.Provider value={streaming}>
      <ReactMarkdown remarkPlugins={remarkPlugins} components={components}>
        {text}
      </ReactMarkdown>
    </StreamingContext.Provider>
  );
});

/**
 * A message as markdown, one top-level block at a time (see `splitBlocks`): while it streams,
 * only the last block re-renders, and code blocks get highlighted once they're complete.
 */
export const Markdown = memo(function Markdown({
  text,
  dark,
  streaming = false,
}: {
  text: string;
  dark: boolean;
  streaming?: boolean;
}) {
  const blocks = useMemo(() => splitBlocks(text), [text]);
  const components = dark ? componentsDark : componentsLight;
  return (
    <div className="cw-md">
      {blocks.map((b, i) => (
        <Block
          key={i}
          text={b}
          components={components}
          streaming={streaming && i === blocks.length - 1}
        />
      ))}
    </div>
  );
});
