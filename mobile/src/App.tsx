import { Suspense, lazy, useEffect, useMemo, useRef, useState } from "react";
import type { DesktopClient } from "./client";
import { RelayClient } from "./connection";
import { go, useClient, useRoute } from "./hooks";
import { MockClient } from "./mock";
import { MultiClient } from "./multi";
import { activePairing, addPairing, loadPairings, parsePairPayload } from "./pairing";
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
/** `?mock=2`: two fake machines, to try the combined view. */
const MOCK_MACHINES = Number(new URLSearchParams(location.search).get("mock")) || 1;

/** Every paired machine at once: one relay connection each, merged into one view. */
function makeClient(): DesktopClient | null {
  if (MOCK) {
    if (MOCK_MACHINES < 2) return new MockClient();
    const members = Array.from({ length: MOCK_MACHINES }, (_, i) => ({ id: `mock${i}`, name: i ? `Mock server ${i}` : "Mock desktop", client: new MockClient(i ? `Mock server ${i}` : "Mock desktop") }));
    members.forEach((m, i) => (m.client.themed = i === 0));
    return new MultiClient(members, "mock0");
  }
  const primary = activePairing();
  if (!primary) return null;
  const all = loadPairings();
  if (all.length < 2) return new RelayClient(primary);
  return new MultiClient(
    all.map((p) => {
      const client = new RelayClient(p);
      client.themed = p.deviceId === primary.deviceId;
      return { id: p.deviceId, name: p.name, client };
    }),
    primary.deviceId,
  );
}

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

  // Rebuilt when pairings change (pair, unpair, another primary): `version` bumps then.
  const client = useMemo<DesktopClient | null>(makeClient, [version]);
  // Close a client only once it's been replaced: StrictMode's rehearsal cleanup must not close the live one.
  const live = useRef(client);
  useEffect(() => {
    live.current = client;
    return () => void setTimeout(() => live.current !== client && client?.close());
  }, [client]);

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
  return <Screens client={client} mock={MOCK} onChanged={changed} />;
}

function Screens({ client, mock, onChanged }: { client: DesktopClient; mock: boolean; onChanged: () => void }) {
  const route = useRoute();
  useClient(client); // a notification's bare pane id resolves once its machine lists it
  const paneId = route.name === "pane" ? (client.canonicalId?.(route.id) ?? route.id) : null;
  const screen =
    route.name === "settings" ? (
      <Settings client={client} mock={mock} onChanged={onChanged} />
    ) : route.name === "usage" ? (
      <Usage client={client} />
    ) : route.name === "panel" ? (
      <Panel client={client} />
    ) : route.name === "browser" ? (
      <Browser key={route.workspaceId} client={client} workspaceId={route.workspaceId} />
    ) : route.name === "android" ? (
      <Android key={route.workspaceId ?? ""} client={client} workspaceId={route.workspaceId} />
    ) : route.name === "terminal" ? (
      <Suspense fallback={<div className="screen panel-screen" />}>
        <Term key={route.paneId} client={client} workspaceId={route.workspaceId} paneId={route.paneId} />
      </Suspense>
    ) : route.name === "files" ? (
      <Files key={route.workspaceId + (route.path ?? "")} client={client} workspaceId={route.workspaceId} path={route.path} />
    ) : route.name === "new" ? (
      <NewSession client={client} workspaceId={route.workspaceId} />
    ) : paneId ? (
      <Thread key={paneId} client={client} paneId={paneId} />
    ) : (
      <Inbox client={client} />
    );
  return (
    <>
      {screen}
      {(route.name === "inbox" || route.name === "settings" || route.name === "panel") && <TabBar client={client} active={route.name} />}
      <Island client={client} openPaneId={paneId} />
    </>
  );
}
