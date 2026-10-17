import { test } from "node:test";
import assert from "node:assert/strict";
import { parseArgs, ArgsError } from "../src/args.js";

const spec = () => ({
  verbose: { type: "boolean", alias: "v" },
  force: { type: "boolean", alias: "f" },
  color: { type: "boolean", default: true },
  num: { type: "number", alias: "n", default: 1 },
  out: { type: "string", alias: "o" },
  include: { type: "string", alias: "I", multiple: true },
  level: { type: "number", multiple: true, alias: "l" },
});

const isArgsError = (re) => (e) => {
  assert.ok(e instanceof ArgsError, `expected ArgsError, got ${e && e.constructor && e.constructor.name}: ${e && e.message}`);
  assert.ok(e instanceof Error);
  assert.equal(e.name, "ArgsError");
  if (re) assert.match(e.message, re);
  return true;
};

test("README example", () => {
  const r = parseArgs(["-vn", "3", "--out=a.txt", "in.txt"], {
    verbose: { type: "boolean", alias: "v" },
    num: { type: "number", alias: "n", default: 1 },
    out: { type: "string" },
    include: { type: "string", alias: "I", multiple: true },
  });
  assert.deepEqual(r, { verbose: true, num: 3, out: "a.txt", include: [], _: ["in.txt"] });
});

test("defaults and absent keys", () => {
  const r = parseArgs([], spec());
  assert.deepEqual(r, { verbose: false, force: false, color: true, num: 1, include: [], level: [], _: [] });
  assert.equal("out" in r, false);
  assert.equal("v" in r, false);
});

test("long forms", () => {
  const r = parseArgs(["--verbose", "--no-color", "--num", "-5", "--out", "--x", "pos"], spec());
  assert.equal(r.verbose, true);
  assert.equal(r.color, false);
  assert.equal(r.num, -5);
  assert.equal(r.out, "--x");
  assert.deepEqual(r._, ["pos"]);
  const s = parseArgs(["--out=", "--num=2.5", "--out=a=b"], spec());
  assert.equal(s.out, "a=b");
  assert.equal(s.num, 2.5);
  assert.equal(parseArgs(["--out="], spec()).out, "");
  assert.equal(parseArgs(["--num=0x10"], spec()).num, 16);
});

test("boolean =true/=false and errors", () => {
  assert.equal(parseArgs(["--color=false"], spec()).color, false);
  assert.equal(parseArgs(["--force=true"], spec()).force, true);
  assert.throws(() => parseArgs(["--force=yes"], spec()), isArgsError());
});

test("--no- only for booleans", () => {
  assert.equal(parseArgs(["--no-verbose"], spec()).verbose, false);
  assert.throws(() => parseArgs(["--no-out"], spec()), isArgsError(/--no-out/));
  assert.throws(() => parseArgs(["--no-such"], spec()), isArgsError(/--no-such/));
});

test("short and bundles", () => {
  let r = parseArgs(["-vf"], spec());
  assert.equal(r.verbose && r.force, true);
  r = parseArgs(["-vofile.txt", "x"], spec());
  assert.deepEqual([r.verbose, r.force, r.out, r._], [true, false, "file.txt", ["x"]]);
  r = parseArgs(["-fvo", "file.txt"], spec());
  assert.deepEqual([r.verbose, r.force, r.out, r._], [true, true, "file.txt", []]);
  r = parseArgs(["-ovf"], spec());
  assert.deepEqual([r.verbose, r.out], [false, "vf"]);
  r = parseArgs(["-n", "7", "-n8"], spec());
  assert.equal(r.num, 8);
  r = parseArgs(["-o=x"], spec());
  assert.equal(r.out, "=x");
  r = parseArgs(["-o", "-v"], spec());
  assert.deepEqual([r.out, r.verbose], ["-v", false]);
});

test("unknown short in bundle names the letter", () => {
  assert.throws(() => parseArgs(["-vqf"], spec()), isArgsError(/-q\b/));
  assert.throws(() => parseArgs(["-q"], spec()), isArgsError(/-q/));
  assert.throws(() => parseArgs(["--colr"], spec()), isArgsError(/--colr/));
  assert.throws(() => parseArgs(["--colr=1"], spec()), isArgsError(/--colr/));
});

test("terminator and dash", () => {
  const r = parseArgs(["a", "-", "--", "-v", "--out", "--", "b"], spec());
  assert.deepEqual(r._, ["a", "-", "-v", "--out", "--", "b"]);
  assert.equal(r.verbose, false);
  assert.equal("out" in r, false);
  assert.deepEqual(parseArgs(["--out", "--", "x"], spec()), { ...parseArgs([], spec()), out: "--", _: ["x"] });
});

test("negative positional is an unknown option", () => {
  assert.throws(() => parseArgs(["-5"], spec()), isArgsError(/-5/));
});

test("multiple", () => {
  const r = parseArgs(["-I", "a", "--include=b", "-Ic", "--include", "d", "-l1", "--level", "2"], spec());
  assert.deepEqual(r.include, ["a", "b", "c", "d"]);
  assert.deepEqual(r.level, [1, 2]);
  const sp = { tag: { type: "string", multiple: true, default: ["x", "y"] } };
  const d = parseArgs([], sp);
  assert.deepEqual(d.tag, ["x", "y"]);
  d.tag.push("z");
  assert.deepEqual(sp.tag.default, ["x", "y"]);
  assert.deepEqual(parseArgs(["--tag", "q"], sp).tag, ["q"], "given values replace the default");
  assert.deepEqual(sp.tag.default, ["x", "y"]);
});

test("last wins for non-multiple", () => {
  const r = parseArgs(["--out", "a", "-o", "b", "--verbose", "--no-verbose"], spec());
  assert.equal(r.out, "b");
  assert.equal(r.verbose, false);
});

test("missing values", () => {
  assert.throws(() => parseArgs(["--out"], spec()), isArgsError(/--out/));
  assert.throws(() => parseArgs(["-vo"], spec()), isArgsError(/-o/));
  assert.throws(() => parseArgs(["-n"], spec()), isArgsError(/-n/));
});

test("invalid numbers", () => {
  for (const bad of [["--num", "abc"], ["--num="], ["-n", "1x"], ["-l", "NaN"]]) {
    assert.throws(() => parseArgs(bad, spec()), isArgsError(), JSON.stringify(bad));
  }
  assert.equal(parseArgs(["--num", "1e3"], spec()).num, 1000);
});

test("typed defaults validated before argv", () => {
  assert.throws(() => parseArgs([], { n: { type: "number", default: "1" } }), TypeError);
  assert.throws(() => parseArgs([], { b: { type: "boolean", default: 0 } }), TypeError);
  assert.throws(() => parseArgs([], { s: { type: "string", default: 5 } }), TypeError);
  assert.throws(() => parseArgs(["--zzz"], { s: { type: "string", multiple: true, default: "a" } }), TypeError);
  assert.throws(() => parseArgs([], { s: { type: "number", multiple: true, default: [1, "2"] } }), TypeError);
  assert.equal(parseArgs([], { s: { type: "string", default: "" } }).s, "");
  assert.equal(parseArgs([], { b: { type: "boolean", default: false } }).b, false);
});

test("positionals stay strings; numbers not coerced", () => {
  const r = parseArgs(["10", "x", "--num", "3", "20"], spec());
  assert.deepEqual(r._, ["10", "x", "20"]);
  assert.equal(typeof r.num, "number");
});

test("option names like toString aren't magic", () => {
  assert.throws(() => parseArgs(["--toString"], spec()), isArgsError(/--toString/));
  assert.throws(() => parseArgs(["--constructor"], spec()), isArgsError());
});
