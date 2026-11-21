import type { ReactNode, RefObject } from "react";
import { Icon, type IconName } from "./common";

/**
 * The send button's face: a small nebula, as the desktop paints it (`nebula_send`). It glows when
 * there's something to send and drifts, rose-tinted, while the agent works (then it stops it).
 */
export function NebulaSend({ mode, disabled, onClick, label }: { mode: "send" | "stop" | "idle"; disabled?: boolean; onClick: () => void; label?: string }) {
  return (
    <button className="nebula" data-mode={mode} disabled={disabled} aria-label={label ?? (mode === "stop" ? "Interrupt" : "Send")} onClick={onClick}>
      <span className="nebula-face" aria-hidden />
      <Icon name={mode === "stop" ? "stop" : "send"} size={mode === "stop" ? 13 : 15} className="nebula-icon" />
    </button>
  );
}

/** A low pill under the composer's text: an icon and a short label (the desktop's `pill`). */
export function Pill({ icon, label, onClick, tone, title, disabled }: { icon?: ReactNode; label?: ReactNode; onClick?: () => void; tone?: "warn" | "accent"; title?: string; disabled?: boolean }) {
  const content = (
    <>
      {icon}
      {label != null && <span className="pill-label">{label}</span>}
    </>
  );
  return onClick ? (
    <button className="pill" data-tone={tone} title={title} aria-label={title} disabled={disabled} onClick={onClick}>
      {content}
    </button>
  ) : (
    <span className="pill static" data-tone={tone} title={title}>
      {content}
    </span>
  );
}

export const PillIcon = ({ name }: { name: IconName }) => <Icon name={name} size={14} />;

/** The floating composer card: the text, then a row of pills and the send button. */
export function Composer({
  value,
  onChange,
  onSend,
  placeholder,
  mode,
  disabled,
  busy,
  pills,
  inputRef,
  above,
}: {
  value: string;
  onChange: (v: string) => void;
  onSend: () => void;
  placeholder: string;
  mode: "send" | "stop" | "idle";
  disabled?: boolean;
  busy?: boolean;
  pills?: ReactNode;
  inputRef?: RefObject<HTMLTextAreaElement | null>;
  above?: ReactNode;
}) {
  return (
    <div className="composer" data-busy={busy || undefined}>
      {above}
      <textarea
        ref={inputRef}
        rows={1}
        value={value}
        enterKeyHint="send"
        placeholder={placeholder}
        onChange={(e) => {
          onChange(e.target.value);
          e.target.style.height = "auto";
          e.target.style.height = `${Math.min(e.target.scrollHeight, 160)}px`;
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) onSend();
        }}
      />
      <div className="composer-bar">
        <div className="pills">{pills}</div>
        <NebulaSend mode={mode} disabled={disabled || mode === "idle"} onClick={onSend} />
      </div>
    </div>
  );
}
