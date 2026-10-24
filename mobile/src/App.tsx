import { useEffect, useMemo, useState } from "react";
import type { DesktopClient } from "./client";
import { RelayClient } from "./connection";
import { go, useClient, useRoute } from "./hooks";
import { MockClient } from "./mock";
import { activePairing, addPairing, parsePairPayload } from "./pairing";
import { Inbox } from "./screens/Inbox";
import { NewSession } from "./screens/NewSession";
import { Island } from "./screens/Island";
import { TabBar } from "./screens/TabBar";
import { Pair } from "./screens/Pair";
import { Settings } from "./screens/Settings";
import { Thread } from "./screens/Thread";
import { CodeThread } from "./screens/CodeThread";

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

/** winfer code panes show the desktop's own web UI; other agents the chat view. */
function PaneScreen({ client, paneId }: { client: DesktopClient; paneId: string }) {
  const code = useClient(client).panes.find((p) => p.id === paneId)?.codeWeb;
  return code ? <CodeThread client={client} paneId={paneId} /> : <Thread client={client} paneId={paneId} />;
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
    ) : route.name === "new" ? (
      <NewSession client={client} workspaceId={route.workspaceId} />
    ) : route.name === "pane" ? (
      <PaneScreen key={route.id} client={client} paneId={route.id} />
    ) : (
      <Inbox client={client} />
    );
  return (
    <>
      {screen}
      {route.name !== "pane" && route.name !== "new" && <TabBar client={client} active={route.name === "settings" ? "settings" : "inbox"} />}
      <Island client={client} openPaneId={route.name === "pane" ? route.id : null} />
    </>
  );
}
