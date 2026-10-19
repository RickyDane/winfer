import { b64uDecode } from "./b64";

export interface Pairing {
  deviceId: string;
  roomId: string;
  key: string;
  name: string;
  origin: string;
  createdAt: number;
}

const LIST = "winfer.pairings";
const ACTIVE = "winfer.active";
const B64U = /^[A-Za-z0-9_-]+$/;

export function loadPairings(): Pairing[] {
  try {
    const v = JSON.parse(localStorage.getItem(LIST) || "[]");
    return Array.isArray(v) ? v : [];
  } catch {
    return [];
  }
}

function save(list: Pairing[]) {
  localStorage.setItem(LIST, JSON.stringify(list));
}

export function activeId(): string | null {
  return localStorage.getItem(ACTIVE);
}

export function setActive(deviceId: string) {
  localStorage.setItem(ACTIVE, deviceId);
}

export function activePairing(): Pairing | null {
  const list = loadPairings();
  return list.find((p) => p.deviceId === activeId()) ?? list[0] ?? null;
}

export function addPairing(p: Pairing) {
  save([...loadPairings().filter((x) => x.deviceId !== p.deviceId && x.roomId !== p.roomId), p]);
  setActive(p.deviceId);
}

export function removePairing(deviceId: string) {
  const list = loadPairings().filter((p) => p.deviceId !== deviceId);
  save(list);
  if (activeId() === deviceId) {
    if (list[0]) setActive(list[0].deviceId);
    else localStorage.removeItem(ACTIVE);
  }
}

/** Parse the `p=` payload of a pair link (or a whole pasted link). Throws a readable error. */
export function parsePairPayload(input: string, origin: string): Pairing {
  let payload = input.trim();
  let linkOrigin = origin;
  const m = payload.match(/[#&?]p=([A-Za-z0-9_-]+)/);
  if (m) {
    try {
      linkOrigin = new URL(payload).origin;
    } catch {
      /* bare fragment */
    }
    payload = m[1];
  }
  let json: { v?: number; r?: string; k?: string; d?: string; n?: string };
  try {
    json = JSON.parse(new TextDecoder().decode(b64uDecode(payload)));
  } catch {
    throw new Error("That doesn't look like a winfer pair link.");
  }
  if (json.v !== 1) throw new Error("This pair link is from a newer winfer. Update the app.");
  const { r, k, d, n } = json;
  if (![r, k, d].every((x) => typeof x === "string" && B64U.test(x)) || b64uDecode(k!).length !== 32) {
    throw new Error("Pair link is incomplete. Generate a new one on your desktop.");
  }
  return {
    deviceId: d!,
    roomId: r!,
    key: k!,
    name: typeof n === "string" && n.trim() ? n.trim().slice(0, 60) : "winfer desktop",
    origin: linkOrigin,
    createdAt: Date.now(),
  };
}

/** Where the relay lives: dev override, else the origin the pair link came from. */
export function relayOrigin(p: Pairing): string {
  return localStorage.getItem("relayOrigin") || p.origin || location.origin;
}
