// Dependency-free smoke check for hosts that cannot run the full Vitest suite.
import assert from "node:assert/strict";
import { activate, buildCommand, validateRequest } from "../plugins/cosmos-db/index.js";

const tools = {};
let approval = true, asks = 0, executions = 0;
const connection = { key: "COSMOS_MAIN", updatedAt: 1, meta: { name: "Main", kind: "cosmos", auth: "connection-string" } };
let last;
activate({
  dir: "/plugins/cosmos-db", registerAgentTool: (tool) => { tools[tool.name] = tool; }, registerTool() {}, registerPage() {},
  confirm: async () => { asks++; return approval; },
  vault: {
    list: async () => [structuredClone(connection)],
    exec: async (opts) => { executions++; last = opts; return { code: 0, output: '{"ok":true}' }; },
  },
});
const base = { connection: "Main", database: "app", container: "items" };
assert.equal(JSON.parse(await tools.cosmos_list.run())[0].approveWrites, true);
await tools.cosmos_read.run({ ...base, action: "query", query: "SELECT * FROM c" });
assert.equal(asks, 0);
assert.deepEqual(last.keys, ["COSMOS_MAIN"]);
for (const action of ["create", "upsert", "replace", "delete"]) {
  const request = { ...base, action, id: "1", item: { id: "1" }, partition_key: null };
  const before = executions;
  approval = false;
  await assert.rejects(tools.cosmos_write.run(request), /declined/);
  assert.equal(executions, before);
  approval = true;
  await tools.cosmos_write.run(request);
  assert.equal(executions, before + 1);
}
assert.throws(() => tools.cosmos_read.run({ ...base, action: "delete" }), /Writes must/);
connection.meta.approveWrites = false;
connection.meta.auth = "entra";
const before = asks;
await tools.cosmos_write.run({ ...base, action: "create", item: { id: "1" } });
assert.equal(asks, before);
assert.deepEqual(last.keys, []);
assert.throws(() => validateRequest({ ...base, action: "delete", id: "1" }), /partition_key/);
assert.throws(() => validateRequest({ ...base, action: "query", query: "DELETE FROM c" }), /SELECT/);
const cmd = buildCommand("C:/a'b/cosmos", { ...base, text: "你好'; $(bad)" }, true);
const script = Buffer.from(cmd.split(" ").at(-1), "base64").toString("utf16le");
assert.ok(script.includes("'C:/a''b/cosmos/bridge.py'"));
assert.ok(!script.includes("$(bad)"));
console.log("Cosmos smoke checks passed (reads, all write approvals/denials, policy toggle, identity isolation, validation, command encoding).");
