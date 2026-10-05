import type { Key } from "./i18n.ts";

/**
 * An error named by its catalog key, for code that can't reach the catalogs
 * because node's test runner reads it as is (files/sealed.ts, voice/seal.ts,
 * voice/ogg.ts). Whoever calls that code puts it in words with `inWords`
 * (i18n/i18n.ts) before anyone sees it.
 */
export class Problem extends Error {
  key: Key;
  constructor(key: Key) {
    super(key);
    this.name = "Problem";
    this.key = key;
  }
}
