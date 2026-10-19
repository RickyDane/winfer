import { useState, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import { haptic } from "../hooks";
import { Icon } from "./common";

function textOf(node: ReactNode): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (node && typeof node === "object" && "props" in node) return textOf((node.props as { children?: ReactNode }).children);
  return "";
}

/** "language-ts" on the inner <code>, else "code". */
function lang(node: ReactNode): string {
  const child = Array.isArray(node) ? node[0] : node;
  const cls = child && typeof child === "object" && "props" in child ? (child.props as { className?: string }).className : undefined;
  return cls?.match(/language-(\w+)/)?.[1] ?? "code";
}

function Code({ children }: { children?: ReactNode }) {
  const [done, setDone] = useState(false);
  return (
    <div className="code">
      <div className="code-head">
        <span>{lang(children)}</span>
        <button
        className="code-copy"
        data-done={done}
        onClick={() => {
          void navigator.clipboard?.writeText(textOf(children).replace(/\n$/, "")).then(() => {
            haptic(6);
            setDone(true);
            setTimeout(() => setDone(false), 1400);
          });
        }}
      >
          <Icon name={done ? "check" : "copy"} size={12} /> {done ? "Copied" : "Copy"}
        </button>
      </div>
      <pre>{children}</pre>
    </div>
  );
}

// react-markdown never renders raw HTML (no rehype-raw), so agent output can't inject markup.
export function Markdown({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown
        components={{
          a: ({ href, children }) => (
            <a href={href} target="_blank" rel="noopener noreferrer">
              {children}
            </a>
          ),
          pre: ({ children }) => <Code>{children}</Code>,
          img: ({ alt }) => <span className="sub">[image{alt ? `: ${alt}` : ""}]</span>,
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}
