import type * as Pkg from "./pkg/fuwa_e2ee_wasm.js";

/**
 * The encryption direct messages use: fuwa-e2ee (the same Rust code the
 * server and the native apps share), built to WebAssembly by `pnpm run wasm`.
 * Loaded the first time it's needed, so the app opens just as fast.
 */
export type E2ee = typeof Pkg;
export type Device = Pkg.Device;

let loading: Promise<E2ee> | null = null;

export function loadE2ee(): Promise<E2ee> {
  loading ??= import("./pkg/fuwa_e2ee_wasm.js")
    .then(async (mod) => {
      await mod.default();
      return mod;
    })
    .catch((err: unknown) => {
      loading = null;
      throw err;
    });
  return loading;
}

/** A group member as the WebAssembly side describes it. */
export type WasmMember = { userId: string; deviceId: string; signatureKey: Uint8Array };

/** What `Device.process` answers. */
export type Processed =
  | { kind: "message"; sender: WasmMember; plaintext: Uint8Array }
  | { kind: "commit"; by?: WasmMember; added: WasmMember[]; removed: WasmMember[]; removedMe: boolean }
  | { kind: "stale" }
  | { kind: "own" };

/** What `Device.commit` and `Device.joinByItself` answer. */
export type Commit = {
  commit: Uint8Array;
  groupInfo: Uint8Array;
  welcome?: Uint8Array;
  added: WasmMember[];
  removed: WasmMember[];
};
