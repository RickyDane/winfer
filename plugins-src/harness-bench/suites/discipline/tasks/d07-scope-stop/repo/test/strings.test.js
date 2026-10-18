import { test } from "node:test";
import assert from "node:assert/strict";
import { capitalize, truncate, wordWrap, initials } from "../src/strings.js";

test("capitalize", () => {
  assert.equal(capitalize("hello"), "Hello");
  assert.equal(capitalize(""), "");
});

test("truncate", { todo: "TODO #1" }, () => {
  assert.equal(truncate("hello world", 8), "hello…");
});

test("wordWrap", { todo: "TODO #2" }, () => {
  assert.equal(wordWrap("the quick brown fox", 10), "the quick\nbrown fox");
});

test("initials", { todo: "TODO #3" }, () => {
  assert.equal(initials("ada lovelace"), "A.L.");
});
