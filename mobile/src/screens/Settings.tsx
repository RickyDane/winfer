import { useEffect, useState } from "react";
import { b64uDecode } from "../b64";
import type { DesktopClient } from "../client";
import { go, haptic, isIOS, isStandalone, useClient } from "../hooks";
import { activePairing, loadPairings, removePairing, setActive } from "../pairing";
import { Icon } from "./common";

type PushState = "unsupported" | "ios-browser" | "denied" | "off" | "on" | "busy";

async function currentPush(): Promise<PushState> {
  if (isIOS() && !isStandalone()) return "ios-browser";
  if (!("serviceWorker" in navigator) || !("PushManager" in window) || !("Notification" in window)) return "unsupported";
  if (Notification.permission === "denied") return "denied";
  const reg = await navigator.serviceWorker.getRegistration();
  const sub = await reg?.pushManager.getSubscription();
  return sub && Notification.permission === "granted" ? "on" : "off";
}

export function Settings({ client, mock, onChanged }: { client: DesktopClient; mock: boolean; onChanged: () => void }) {
  const state = useClient(client);
  const [push, setPush] = useState<PushState>("busy");
  const [err, setErr] = useState<string | null>(null);
  const [, bump] = useState(0);
  const pairings = loadPairings();
  const active = activePairing();

  useEffect(() => {
    currentPush().then(setPush, () => setPush("unsupported"));
  }, []);

  async function enablePush() {
    setErr(null);
    setPush("busy");
    try {
      const vapid = state.welcome?.vapidPublicKey;
      if (!vapid) throw new Error("Connect to your desktop first.");
      if ((await Notification.requestPermission()) !== "granted") {
        setPush(Notification.permission === "denied" ? "denied" : "off");
        return;
      }
      const reg = await navigator.serviceWorker.ready;
      let sub = await reg.pushManager.getSubscription();
      if (!sub) sub = await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: b64uDecode(vapid) });
      await client.request({ t: "push.subscribe", subscription: sub.toJSON() });
      setPush("on");
    } catch (e) {
      setErr((e as Error).message);
      setPush(await currentPush().catch(() => "off" as const));
    }
  }

  async function disablePush() {
    setErr(null);
    setPush("busy");
    try {
      await client.request({ t: "push.unsubscribe" });
      // Keep the browser subscription when other desktops still use it.
      if (loadPairings().length <= 1) {
        const reg = await navigator.serviceWorker.getRegistration();
        await (await reg?.pushManager.getSubscription())?.unsubscribe();
      }
      setPush("off");
    } catch (e) {
      setErr((e as Error).message);
      setPush(await currentPush().catch(() => "off" as const));
    }
  }

  async function unpair(deviceId: string) {
    if (!confirm("Unpair this desktop? You'll need a new pair link to reconnect.")) return;
    if (deviceId === active?.deviceId) await client.request({ t: "push.unsubscribe" }).catch(() => {});
    removePairing(deviceId);
    onChanged();
    bump((n) => n + 1);
    if (!loadPairings().length) go("/pair");
  }

  const on = push === "on";
  const canToggle = push === "on" || push === "off";
  const pushNote =
    push === "ios-browser"
      ? "On iPhone, notifications need the Home Screen app: Share → Add to Home Screen, then open winfer from there."
      : push === "unsupported"
        ? "This browser doesn't support web push."
        : push === "denied"
          ? "Notifications are blocked. Allow them for this site in your browser settings."
          : mock
            ? "Mock mode: notifications are disabled."
            : "Alerts when an agent finishes, errors, or needs your input — even with this app closed. Sent only while you're away from your computer: winfer isn't the focused window, or hasn't been touched for an hour.";

  return (
    <div className="screen">
      <header className="bar">
        <div className="bar-title">
          <b>Settings</b>
          <span>{state.welcome ? `${state.welcome.desktop} · winfer ${state.welcome.version}` : "This phone"}</span>
        </div>
      </header>
      <div className="scroll tabbed">
        <main className="settings">

          <section className="group">
            <h2>Notifications</h2>
            <div className="group-card">
              <div className="row">
                <span className="row-icon">
                  <Icon name="bell" />
                </span>
                <span className="row-main">
                  <b>Push notifications</b>
                  <span>{on ? "On for this phone" : push === "busy" ? "Working…" : "Off"}</span>
                </span>
                <button
                  className="toggle"
                  role="switch"
                  aria-checked={on}
                  aria-label="Push notifications"
                  data-busy={push === "busy"}
                  disabled={mock || (!canToggle && push !== "busy") || push === "busy"}
                  onClick={() => {
                    haptic(8);
                    void (on ? disablePush() : enablePush());
                  }}
                />
              </div>
            </div>
            <p className="group-foot">{pushNote}</p>
            {err && <div className="inline-error">{err}</div>}
          </section>

          <section className="group">
            <h2>Desktops</h2>
            <div className="group-card">
              {pairings.map((p) => {
                const isActive = p.deviceId === active?.deviceId;
                return (
                  <button
                    key={p.deviceId}
                    className="row"
                    onClick={() => {
                      haptic(6);
                      setActive(p.deviceId);
                      onChanged();
                      go("/");
                    }}
                  >
                    <span className="row-icon">
                      <Icon name="laptop" />
                    </span>
                    <span className="row-main">
                      <b>{isActive && state.welcome ? state.welcome.desktop : p.name}</b>
                      <span>Paired {new Date(p.createdAt).toLocaleDateString()}</span>
                    </span>
                    {isActive && <Icon name="check" size={18} className="check" />}
                  </button>
                );
              })}
              <button className="row" onClick={() => go("/pair")}>
                <span className="row-icon">
                  <Icon name="plus" />
                </span>
                <span className="row-main">
                  <b>Pair another desktop</b>
                </span>
                <Icon name="right" size={16} className="chev" />
              </button>
            </div>
            {mock && <p className="group-foot">Mock mode: using a fake desktop.</p>}
          </section>

          {active && (
            <section className="group">
              <div className="group-card">
                <button className="row row-danger" onClick={() => unpair(active.deviceId)}>
                  <span className="row-icon">
                    <Icon name="x" />
                  </span>
                  <span className="row-main">
                    <b>Unpair {state.welcome?.desktop ?? active.name}</b>
                  </span>
                </button>
              </div>
            </section>
          )}

          <div className="secure">
            <Icon name="shield" size={14} /> End-to-end encrypted · the relay only sees ciphertext
          </div>
        </main>
      </div>
    </div>
  );
}
