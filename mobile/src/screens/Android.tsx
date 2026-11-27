// The desktop's device mirror on the phone (Android devices and emulators, a Mac's iOS
// Simulators): pick a device or start one, watch it live, touch and swipe it, its buttons and typing.
import { useEffect, useState } from "react";
import type { DesktopClient } from "../client";
import { haptic, useClient } from "../hooks";
import { forMachine } from "../multi";
import type { AndroidOp, AndroidState, IosSimulator } from "../protocol";
import { Icon, linkState } from "./common";
import { LiveView, type Unit } from "./LiveView";
import { BarButton, ComposeBar, Notice, PanelBar, call, useFrames, usePoll } from "./panelKit";

const KEY = { back: 4, home: 3, recents: 187, power: 26, enter: 66, del: 67, tab: 61, up: 19, down: 20, left: 21, right: 22, volUp: 24, volDown: 25 };

const MODE: Record<string, string> = { scrcpy: "live", live: "live", stream: "screen recording", screenshots: "screenshots" };

type Device = { id: string; name: string; detail: string; ios: boolean };

/** The devices to mirror (connected Android ones, booted simulators) and the ones to start. */
export function devicesOf(as: AndroidState | null): { ready: Device[]; startable: Device[] } {
  const sims = as?.ios?.devices ?? [];
  const sim = (d: IosSimulator, detail: string): Device => ({ id: d.udid, name: d.name, detail: `${detail} · ${d.runtime ?? "iOS"}`, ios: true });
  return {
    ready: [
      ...(as?.devices ?? []).filter((d) => d.state === "device").map((d) => ({ id: d.serial, name: `${d.model || d.serial}${d.emulator ? " (emulator)" : ""}`, detail: d.serial, ios: false })),
      ...sims.filter((d) => d.booted).map((d) => sim(d, "Simulator")),
    ],
    startable: [
      ...(as?.avds ?? []).map((name) => ({ id: name, name: name.replace(/_/g, " "), detail: "Start this emulator", ios: false })),
      ...sims.filter((d) => !d.booted).map((d) => sim(d, "Start this simulator")),
    ],
  };
}

export function Android({ client, workspaceId }: { client: DesktopClient; workspaceId: string | null }) {
  const state = useClient(client);
  // The devices of the machine of the workspace it was opened from (else the primary machine's),
  // when several are paired: its calls carry the workspace, which routes them there.
  const ws = workspaceId ? state.workspaces.find((w) => w.id === workspaceId) : undefined;
  const machine = ws?.machine ?? state.machines.find((m) => m.primary)?.id;
  const machineName = state.machines.length > 1 ? state.machines.find((m) => m.id === machine)?.name : undefined;
  const online = linkState(forMachine(state, machine)) === "online";
  const at = ws ? { workspaceId: ws.id } : {};
  const [as, setAs] = useState<AndroidState | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [typing, setTyping] = useState(false);
  const [picking, setPicking] = useState(false);
  // A simulator started from here, shown once it has booted.
  const [booting, setBooting] = useState<string | null>(null);
  const { frame, drawn } = useFrames(client, "android");

  const op = <T,>(o: AndroidOp, extra: Record<string, unknown> = {}) => call<T>(client, { t: "android.call", op: o, ...at, ...extra });
  const fire = (o: AndroidOp, extra: Record<string, unknown>) => client.fire({ t: "android.call", op: o, ...at, ...extra });
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
      client.fire({ t: "android.call", op: "unwatch", ...at });
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

  const { ready, startable } = devicesOf(as);
  const current = ready.find((d) => d.id === as?.current);
  const mirror = as?.mirror && as.mirror.serial === current?.id ? as.mirror : null;
  const viewOnly = mirror?.input === false;
  const select = (id: string) => {
    setPicking(false);
    setBooting(null);
    void act("select", { serial: id });
  };
  const start = (d: Device) => {
    haptic(5);
    if (d.ios) setBooting(d.id);
    void act(d.ios ? "startSim" : "startAvd", d.ios ? { udid: d.id } : { name: d.id });
  };
  const booted = booting !== null && ready.some((d) => d.id === booting);
  useEffect(() => {
    if (booted) select(booting!);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [booted]);
  // `starting` is an emulator's name or a simulator's udid.
  const starting = as?.starting ? (startable.find((d) => d.id === as.starting)?.name ?? ready.find((d) => d.id === as.starting)?.name ?? as.starting.replace(/_/g, " ")) : null;

  const rows = (title: string, list: Device[], onPick: (d: Device) => void) =>
    list.length > 0 && (
      <section className="group android-avds">
        <h2>{title}</h2>
        <div className="group-card">
          {list.map((d) => (
            <button key={d.id} className="row" aria-pressed={d.id === as?.current} onClick={() => onPick(d)}>
              <span className="row-icon">
                <Icon name="phone" size={15} />
              </span>
              <span className="row-main">
                <b>{d.name}</b>
                <span>{d.detail}</span>
              </span>
              <Icon name="right" size={16} />
            </button>
          ))}
        </div>
      </section>
    );

  const body = () => {
    if (!online) return <Notice icon="phone" title="Desktop offline" />;
    if (!as || !as.loaded) {
      const why = err ?? as?.error;
      return why ? <Notice icon="alert" title="Can't reach the mirror" text={why} /> : <Notice icon="phone" title="Looking for devices…" />;
    }
    if (as.adb === false && !as.ios?.simctl) return <Notice icon="phone" title="No adb or Xcode on the desktop" text="Install the Android SDK platform tools to mirror Android devices and emulators, or Xcode on a Mac for iOS Simulators." />;
    if (!current || picking)
      return (
        <div className="android-empty">
          {(!current || starting || as.error) && (
            <Notice
              icon="phone"
              title={starting ? `Starting ${starting}…` : ready.length ? "Pick a device" : "No device connected"}
              text={as.error ?? (ready.length ? "The desktop's panel shows another machine's device. Pick one of these to mirror it here." : startable.length ? "Start an emulator or simulator, or connect a device to the desktop." : "Connect a device to the desktop, or create an emulator in Android Studio or a simulator in Xcode.")}
            />
          )}
          {rows("Connected", ready, (d) => select(d.id))}
          {!as.starting && rows("Android emulators", startable.filter((d) => !d.ios), start)}
          {!as.starting && rows("iOS Simulators", startable.filter((d) => d.ios), start)}
        </div>
      );
    return (
      <LiveView
        mode="touch"
        frame={frame}
        drawn={drawn}
        onTouch={touch}
        placeholder={<Notice icon="phone" title={mirror?.error ? "The mirror stopped" : "Connecting to the device…"} text={mirror?.error ?? undefined} />}
      />
    );
  };

  const shown = current && !picking;
  return (
    <div className="screen panel-screen">
      <PanelBar title={current?.name ?? "Devices"} subtitle={current ? `${current.detail}${mirror ? ` · ${MODE[mirror.mode]}` : ""}` : machineName ? `Devices on ${machineName}` : "The desktop's device mirror"}>
        <BarButton icon="layers" label="Devices" active={picking} disabled={!current} onClick={() => setPicking((p) => !p)} />
        <BarButton icon="keyboard" label="Type" active={typing} disabled={!shown || viewOnly} onClick={() => setTyping((t) => !t)} />
      </PanelBar>
      {shown && ready.length > 1 && (
        <div className="panel-ws chips">
          {ready.map((d) => (
            <button key={d.id} className="chip" aria-pressed={d.id === as?.current} onClick={() => select(d.id)}>
              {d.name}
            </button>
          ))}
        </div>
      )}
      {err && as && <p className="inline-error panel-error">{err}</p>}
      {shown && viewOnly && <p className="inline-error panel-error">View only: install AXe on the Mac to tap and type (brew install cameroncooke/axe/axe).</p>}
      <div className="panel-stage android-stage">{body()}</div>
      {shown && !viewOnly && typing && (
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
            ...(current.ios
              ? []
              : [
                  { label: "Vol+", title: "Volume up", onPress: () => key(KEY.volUp) },
                  { label: "Vol−", title: "Volume down", onPress: () => key(KEY.volDown) },
                ]),
          ]}
        />
      )}
      {shown && !viewOnly && (
        <nav className="android-keys" aria-label="Device buttons">
          {!current.ios && <BarButton icon="back" label="Back" onClick={() => key(KEY.back)} />}
          <BarButton icon="home" label="Home" onClick={() => key(KEY.home)} />
          {!current.ios && <BarButton icon="layers" label="Recent apps" onClick={() => key(KEY.recents)} />}
          <BarButton icon="power" label={current.ios ? "Lock" : "Power"} onClick={() => key(KEY.power)} />
        </nav>
      )}
    </div>
  );
}
