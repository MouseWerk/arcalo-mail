import { test } from "node:test";
import assert from "node:assert/strict";
import { problems } from "./commit-rules.mjs";

const owner = { name: "Maurice Kleindienst", email: "kleindiema@gmail.com" };

test("the owner's plain commits pass", () => {
  assert.deepEqual(problems([{ id: "abc1234", message: "Sync: fetch the newest headers first\n\nDetails.", author: owner }]), []);
  assert.deepEqual(problems([]), []);
});

test("trailers, generated lines and other authors fail", () => {
  assert.equal(problems([{ id: "a", message: "Fix\n\nCo-Authored-By: Someone <x@y>", author: owner }]).length, 1);
  assert.equal(problems([{ id: "b", message: "Fix\n\nExample-Session: https://example.org", author: owner }]).length, 1);
  assert.equal(problems([{ id: "c", message: "Fix\n\nGenerated with [Some Tool](https://example.org)", author: owner }]).length, 1);
  assert.equal(problems([{ id: "d", message: "Fix", author: { name: "Someone", email: "x@y" } }]).length, 1);
});
