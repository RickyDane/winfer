import { useState, type CSSProperties } from "react";
import { go, isIOS, isStandalone } from "../hooks";
import { addPairing, parsePairPayload } from "../pairing";
import { Aurora, Icon } from "./common";

export function Pair({ error, canCancel, onPaired }: { error: string | null; canCancel: boolean; onPaired: () => void }) {
  const [link, setLink] = useState("");
  const [err, setErr] = useState<string | null>(error);
  const iosBrowser = isIOS() && !isStandalone();

  function submit() {
    try {
      addPairing(parsePairPayload(link, location.origin));
      onPaired();
    } catch (e) {
      setErr((e as Error).message);
    }
  }

  return (
    <div className="screen">
      <Aurora />
      <div className="scroll">
        <main className="pair">
          <div className="pair-logo" aria-hidden>
            <i />
            <i />
            <img src="./icons/logo.svg" alt="" />
          </div>
          <h1>Your agents, in your pocket</h1>
          <p className="lead">Get pinged the moment an agent finishes or needs you — then answer, approve or steer it from here.</p>
          <ol className="steps">
            <li style={{ "--i": 0 } as CSSProperties}>
              <span className="n">1</span>
              <span>
                On your computer open winfer → <b>Settings → Mobile</b> → <b>Pair phone</b>.
              </span>
            </li>
            <li style={{ "--i": 1 } as CSSProperties}>
              <span className="n">2</span>
              <span>Scan the QR code with your camera — or paste the link below.</span>
            </li>
          </ol>
          <div className="paste">
            <textarea
              rows={3}
              value={link}
              placeholder="https://…/#p=…"
              onChange={(e) => {
                setLink(e.target.value);
                setErr(null);
              }}
            />
            <button className="btn block primary" disabled={!link.trim()} onClick={submit}>
              <Icon name="link" size={18} /> Pair
            </button>
            {canCancel && (
              <button className="btn block ghost" onClick={() => go("/")}>
                Cancel
              </button>
            )}
          </div>
          {err && <div className="inline-error" style={{ marginTop: 12 }}>{err}</div>}
          {iosBrowser && (
            <div className="tip">
              <b>iPhone tip:</b> notifications only work from the Home Screen app. Tap <b>Share</b> → <b>Add to Home Screen</b>, then open winfer from there and pair.
            </div>
          )}
          <div className="secure">
            <Icon name="shield" size={14} /> End-to-end encrypted. The relay only sees ciphertext.
          </div>
        </main>
      </div>
    </div>
  );
}
