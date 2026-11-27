import { Suspense, lazy, useEffect, useMemo, useState } from "react";
import type { DesktopClient } from "./client";
import { RelayClient } from "./connection";
import { go, useRoute } from "./hooks";
import { MockClient } from "./mock";
import { activePairing, addPairing, parsePairPayload } from "./pairing";
import { Inbox } from "./screens/Inbox";
import { NewSession } from "./screens/NewSession";
import { Island } from "./screens/Island";
import { TabBar } from "./screens/TabBar";
import { Pair } from "./screens/Pair";
import { Settings } from "./screens/Settings";
import { Thread } from "./screens/Thread";
import { Usage } from "./screens/Usage";
import { Android } from "./screens/Android";
import { Browser } from "./screens/Browser";
import { Files } from "./screens/Files";
import { Panel } from "./screens/Panel";
// xterm is big: only loaded when a terminal opens.
const Term = lazy(() => import("./screens/Term").then((m) => ({ default: m.Term })));

const MOCK = new URLSearchParams(location.search).has("mock");

/** Consume a `#p=…` pair link: store it, then strip the secret from the URL. */
function consumePairLink(): string | null {
  if (!/^#p=/.test(location.hash)) return null;
  try {
    addPairing(parsePairPayload(location.hash, location.origin));
    history.replaceState(null, "", location.pathname + location.search + "#/");
    return null;
  } catch (e) {
    history.replaceState(null, "", location.pathname + location.search + "#/pair");
    return (e as Error).message;
  }
}

export function App() {
  const [pairError, setPairError] = useState(consumePairLink);
  const [version, setVersion] = useState(0);
  const route = useRoute();

  useEffect(() => {
    // Only a pair link changes the pairing; ordinary navigation must keep the connection.
    const on = () => {
      if (!/^#p=/.test(location.hash)) return;
      const err = consumePairLink();
      setPairError(err);
      if (!err) setVersion((v) => v + 1);
    };
    window.addEventListener("hashchange", on);
    return () => window.removeEventListener("hashchange", on);
  }, []);

  const pairing = MOCK ? null : activePairing();
  const client = useMemo<DesktopClient | null>(
    () => (MOCK ? new MockClient() : pairing ? new RelayClient(pairing) : null),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [pairing?.deviceId, version],
  );
  useEffect(() => () => client?.close(), [client]);

  const changed = () => setVersion((v) => v + 1);

  if (!client || route.name === "pair")
    return (
      <Pair
        error={pairError}
        canCancel={!!client}
        onPaired={() => {
          setPairError(null);
          changed();
          go("/");
        }}
      />
    );
  const screen =
    route.name === "settings" ? (
      <Settings client={client} mock={MOCK} onChanged={changed} />
    ) : route.name === "usage" ? (
      <Usage client={client} />
    ) : route.name === "panel" ? (
      <Panel client={client} />
    ) : route.name === "browser" ? (
      <Browser key={route.workspaceId} client={client} workspaceId={route.workspaceId} />
    ) : route.name === "android" ? (
      <Android client={client} />
    ) : route.name === "terminal" ? (
      <Suspense fallback={<div className="screen panel-screen" />}>
        <Term key={route.paneId} client={client} workspaceId={route.workspaceId} paneId={route.paneId} />
      </Suspense>
    ) : route.name === "files" ? (
      <Files key={route.workspaceId} client={client} workspaceId={route.workspaceId} />
    ) : route.name === "new" ? (
      <NewSession client={client} workspaceId={route.workspaceId} />
    ) : route.name === "pane" ? (
      <Thread key={route.id} client={client} paneId={route.id} />
    ) : (
      <Inbox client={client} />
    );
  return (
    <>
      {screen}
      {(route.name === "inbox" || route.name === "settings" || route.name === "panel") && <TabBar client={client} active={route.name} />}
      <Island client={client} openPaneId={route.name === "pane" ? route.id : null} />
    </>
  );
}
