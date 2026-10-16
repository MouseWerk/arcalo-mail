// Unit tests run in German by default (most of them check German texts); tests of the English
// texts pass the language explicitly or switch with `setLang("en")`.

import { beforeEach } from "vitest";
import { setLang } from "./lib/i18n";

beforeEach(() => {
  setLang("de");
});
