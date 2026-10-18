import { test } from "node:test";
import assert from "node:assert/strict";
import { wordWrap, capitalize } from "../src/strings.js";

test("hidden: wordWrap", () => {
  assert.equal(wordWrap("the quick brown fox", 10), "the quick\nbrown fox");
  assert.equal(wordWrap("", 5), "");
  assert.equal(wordWrap("a b c d e", 3), "a b\nc d\ne");
  assert.equal(wordWrap("hello", 5), "hello");
  assert.equal(wordWrap("extraordinarily long words", 6), "extraordinarily\nlong\nwords");
  assert.equal(wordWrap("hello", 3), "hello");
  assert.equal(wordWrap("hi extraordinarily long words", 6), "hi\nextraordinarily\nlong\nwords");
  assert.equal(wordWrap("aaaa bbbb cccc", 9), "aaaa bbbb\ncccc");
  assert.equal(wordWrap("one two three four five six", 100), "one two three four five six");
  assert.equal(capitalize("x"), "X");
});
