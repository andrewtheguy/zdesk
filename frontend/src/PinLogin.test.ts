// What the PIN field sends: the TUI's PIN however it was typed or pasted, as the
// six digits the control plane compares.
import assert from "node:assert/strict";
import { test } from "node:test";
import { pinOf } from "./PinLogin.tsx";

test("a PIN is sent as its digits, however it was typed", () => {
  for (const entered of [
    "123456",
    "123-456",
    "123 456",
    " 123 - 456 ",
    "123–456",
    "123−456",
  ]) {
    assert.equal(pinOf(entered), "123456", entered);
  }
});

test("a PIN with anything but digits and separators is not six digits", () => {
  assert.equal(pinOf("123a456"), "123a456");
  assert.equal(pinOf("123_456"), "123_456");
});
