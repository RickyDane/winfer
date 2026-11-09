import { describe, expect, it, vi } from "vitest";
import { activate, buildCommand, validateRequest } from "../plugins/cosmos-db/index.js";

const read = { connection: "Main", action: "query", database: "app", container: "items", query: "SELECT * FROM c" };
const write = { ...read, action: "create", item: { id: "1", tenant: "a" } };
function setup(meta = {}) {
  const tools = {};
  const conn = { key: "COSMOS_MAIN", updatedAt: 1, meta: { kind: "cosmos", name: "Main", auth: "connection-string", ...meta } };
  const api = {
    dir: "C:/plugins/cosmos-db", registerAgentTool: (t) => { tools[t.name] = t; }, registerPage: vi.fn(), registerTool: vi.fn(),
    confirm: vi.fn(async () => true),
    vault: { list: vi.fn(async () => [structuredClone(conn)]), exec: vi.fn(async () => ({ code: 0, output: '{"id":"1"}' })) },
  };
  activate(api);
  return { tools, api, conn };
}

describe("Cosmos agent tools", () => {
  it("lists no credentials or endpoint", async () => {
    const { tools } = setup({ endpoint: "secret-endpoint" });
    const text = await tools.cosmos_list.run();
    expect(text).not.toContain("secret-endpoint");
    expect(JSON.parse(text)[0].approveWrites).toBe(true);
  });
  it("reads without approval and passes only the vault key", async () => {
    const { tools, api } = setup();
    await tools.cosmos_read.run(read);
    expect(api.confirm).not.toHaveBeenCalled();
    expect(api.vault.exec.mock.calls[0][0].keys).toEqual(["COSMOS_MAIN"]);
  });
  it.each(["create", "upsert", "replace", "delete"])("requires approval for %s by default", async (action) => {
    const { tools, api } = setup();
    api.confirm.mockResolvedValue(false);
    await expect(tools.cosmos_write.run({ ...write, action, id: "1", partition_key: "a" }, { label: "Agent" })).rejects.toThrow("declined");
    expect(api.confirm).toHaveBeenCalledOnce();
    expect(api.vault.exec).not.toHaveBeenCalled();
  });
  it("skips approval only when explicitly disabled", async () => {
    const { tools, api } = setup({ auth: "entra", approveWrites: false });
    await tools.cosmos_write.run(write);
    expect(api.confirm).not.toHaveBeenCalled();
    expect(api.vault.exec.mock.calls[0][0].keys).toEqual([]);
  });
  it("refuses a policy change during approval", async () => {
    const { tools, api, conn } = setup();
    api.confirm.mockImplementation(async () => { conn.meta.approveWrites = false; return true; });
    await expect(tools.cosmos_write.run(write)).rejects.toThrow("Connection changed");
    expect(api.vault.exec).not.toHaveBeenCalled();
  });
  it("snapshots the approved item", async () => {
    const { tools, api } = setup();
    const args = structuredClone(write);
    api.confirm.mockImplementation(async () => { args.item.id = "changed"; return true; });
    await tools.cosmos_write.run(args);
    const command = api.vault.exec.mock.calls[0][0].command;
    // Node's platform uses POSIX here; the payload is always base64 JSON.
    expect(command).not.toContain("changed");
  });
  it("cannot route writes through the read tool", () => {
    const { tools, api } = setup();
    expect(() => tools.cosmos_read.run(write)).toThrow("Writes must");
    expect(api.vault.exec).not.toHaveBeenCalled();
  });
  it("does not expose raw failed bridge output", async () => {
    const { tools, api } = setup();
    api.vault.exec.mockResolvedValue({ code: 1, output: "AccountKey=secret" });
    await expect(tools.cosmos_read.run(read)).rejects.toThrow("bridge failed");
  });
});

describe("Cosmos validation and commands", () => {
  it("validates required identifiers, page size, queries and item bodies", () => {
    for (const args of [{ ...read, database: "" }, { ...read, max_items: 101 }, { ...read, query: "DELETE FROM c" }, { ...write, item: {} }, { ...write, action: "replace", id: "different" }, { ...read, action: "delete", id: "1" }]) expect(() => validateRequest(args)).toThrow();
    expect(validateRequest({ ...read, action: "read", id: "1", partition_key: null }).partition_key).toBeNull();
  });
  it("encodes Unicode data and quotes paths on both platforms", () => {
    const args = { ...write, item: { id: "你好'; $(bad)" } };
    const win = buildCommand("C:/a'b/cosmos", args, true);
    const script = Buffer.from(win.split(" ").at(-1), "base64").toString("utf16le");
    expect(script).toContain("'C:/a''b/cosmos/bridge.py'");
    const payload = script.match(/bridge.py' '([^']+)'/)[1];
    expect(JSON.parse(Buffer.from(payload, "base64").toString())).toEqual(args);
    expect(buildCommand("/a'b", args, false)).toContain("'/a'\\''b/bridge.py'");
  });
});
