// The desktop's Android mirror on the phone: pick a device or start an emulator, watch it live,
// touch and swipe it, its buttons and typing.
import { useEffect, useState } from "react";
import type { DesktopClient } from "../client";
import { haptic, useClient } from "../hooks";
import { forMachine } from "../multi";
import type { AndroidOp, AndroidState } from "../protocol";
import { Icon, linkState } from "./common";
import { LiveView, type Unit } from "./LiveView";
import { BarButton, ComposeBar, Notice, PanelBar, call, useFrames, usePoll } from "./panelKit";

const KEY = { back: 4, home: 3, recents: 187, power: 26, enter: 66, del: 67, tab: 61, up: 19, down: 20, left: 21, right: 22, volUp: 24, volDown: 25 };

const MODE: Record<string, string> = { scrcpy: "live", stream: "screen recording", screenshots: "screenshots" };

export function Android({ client }: { client: DesktopClient }) {
  const state = useClient(client);
  // The Android panel is the primary machine's, when several are paired.
  const online = linkState(forMachine(state, state.machines.find((m) => m.primary)?.id)) === "online";
  const [as, setAs] = useState<AndroidState | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [typing, setTyping] = useState(false);
  const { frame, drawn } = useFrames(client, "android");

  const op = <T,>(o: AndroidOp, extra: Record<string, unknown> = {}) => call<T>(client, { t: "android.call", op: o, ...extra });
  const fire = (o: AndroidOp, extra: Record<string, unknown>) => client.fire({ t: "android.call", op: o, ...extra });
  const refresh = () =>
    op<AndroidState>("state")
      .then((s) => {
        setAs(s);
        setErr(null);
      })
      .catch((e: Error) => setErr(e.message));
  usePoll(() => online && void refresh(), 2000, [online]);

  // The mirror runs while the phone watches; again after the phone comes back.
  const maxWidth = Math.round(Math.min(1080, innerWidth * Math.min(2, devicePixelRatio || 1)));
  useEffect(() => {
    if (!online) return;
    const watch = () => document.visibilityState === "visible" && op<AndroidState>("watch", { maxWidth }).then(setAs).catch(() => {});
    watch();
    document.addEventListener("visibilitychange", watch);
    return () => {
      document.removeEventListener("visibilitychange", watch);
      client.fire({ t: "android.call", op: "unwatch" });
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [online]);

  const touch = (kind: "down" | "move" | "up", at: Unit) => {
    if (kind === "down") haptic(3);
    fire("touch", { kind, x: at.x, y: at.y });
  };
  const key = (code: number) => {
    haptic(5);
    fire("key", { code });
  };
  const act = (o: AndroidOp, extra: Record<string, unknown>) =>
    op(o, extra)
      .then(refresh)
      .catch((e: Error) => setErr(e.message));

  const ready = (as?.devices ?? []).filter((d) => d.state === "device");
  const current = ready.find((d) => d.serial === as?.current);
  const label = (d: { model?: string; serial: string; emulator?: boolean }) => `${d.model || d.serial}${d.emulator ? " (emulator)" : ""}`;

  const body = () => {
    if (!online) return <Notice icon="phone" title="Desktop offline" />;
    if (!as || !as.loaded) return err ? <Notice icon="alert" title="Can't reach the mirror" text={err} /> : <Notice icon="phone" title="Looking for devices…" />;
    if (as.adb === false) return <Notice icon="phone" title="No adb on the desktop" text="Install the Android SDK platform tools on the desktop to mirror devices and emulators." />;
    if (!current)
      return (
        <div className="android-empty">
          <Notice icon="phone" title={as.starting ? `Starting ${as.starting}…` : "No device connected"} text={as.error ?? (as.avds?.length ? "Start an emulator, or connect a device to the desktop." : "Connect a device to the desktop, or create an emulator in Android Studio.")} />
          {!as.starting && (as.avds ?? []).length > 0 && (
            <div className="group-card android-avds">
              {(as.avds ?? []).map((name) => (
                <button key={name} className="row" onClick={() => void act("startAvd", { name })}>
                  <span className="row-icon">
                    <Icon name="phone" size={15} />
                  </span>
                  <span className="row-main">
                    <b>{name.replace(/_/g, " ")}</b>
                    <span>Start this emulator</span>
                  </span>
                  <Icon name="right" size={16} />
                </button>
              ))}
            </div>
          )}
        </div>
      );
    return (
      <LiveView
        mode="touch"
        frame={frame}
        drawn={drawn}
        onTouch={touch}
        placeholder={<Notice icon="phone" title={as.mirror?.error ? "The mirror stopped" : "Connecting to the device…"} text={as.mirror?.error ?? undefined} />}
      />
    );
  };

  return (
    <div className="screen panel-screen">
      <PanelBar title={current ? label(current) : "Android"} subtitle={current ? `${current.serial}${as?.mirror ? ` · ${MODE[as.mirror.mode]}` : ""}` : "The desktop's Android mirror"}>
        <BarButton icon="keyboard" label="Type" active={typing} disabled={!current} onClick={() => setTyping((t) => !t)} />
      </PanelBar>
      {ready.length > 1 && (
        <div className="panel-ws chips">
          {ready.map((d) => (
            <button key={d.serial} className="chip" aria-pressed={d.serial === as?.current} onClick={() => void act("select", { serial: d.serial })}>
              {label(d)}
            </button>
          ))}
        </div>
      )}
      {err && as && <p className="inline-error panel-error">{err}</p>}
      <div className="panel-stage android-stage">{body()}</div>
      {current && typing && (
        <ComposeBar
          placeholder="Type on the device"
          onText={(text, enter) => {
            if (text) fire("text", { text });
            if (enter) key(KEY.enter);
          }}
          keys={[
            { label: <Icon name="del" size={16} />, title: "Delete", onPress: () => key(KEY.del) },
            { label: "Tab", title: "Tab", onPress: () => key(KEY.tab) },
            { label: "←", title: "Left", onPress: () => key(KEY.left) },
            { label: "↑", title: "Up", onPress: () => key(KEY.up) },
            { label: "↓", title: "Down", onPress: () => key(KEY.down) },
            { label: "→", title: "Right", onPress: () => key(KEY.right) },
            { label: "Vol+", title: "Volume up", onPress: () => key(KEY.volUp) },
            { label: "Vol−", title: "Volume down", onPress: () => key(KEY.volDown) },
          ]}
        />
      )}
      {current && (
        <nav className="android-keys" aria-label="Device buttons">
          <BarButton icon="back" label="Back" onClick={() => key(KEY.back)} />
          <BarButton icon="home" label="Home" onClick={() => key(KEY.home)} />
          <BarButton icon="layers" label="Recent apps" onClick={() => key(KEY.recents)} />
          <BarButton icon="power" label="Power" onClick={() => key(KEY.power)} />
        </nav>
      )}
    </div>
  );
}
