// Builds the encryption the app's direct messages use (the e2ee-wasm crate)
// into src/e2ee/pkg: cargo for wasm32-unknown-unknown, then wasm-bindgen.
// Needs `rustup target add wasm32-unknown-unknown` and the wasm-bindgen CLI
// at the version e2ee-wasm/Cargo.toml pins:
//   cargo install wasm-bindgen-cli --version 0.2.129 --locked
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));
const out = fileURLToPath(new URL("../src/e2ee/pkg", import.meta.url));
const run = (command, args) => execFileSync(command, args, { cwd: root, stdio: "inherit" });

run("cargo", ["build", "--locked", "-p", "fuwa-e2ee-wasm", "--target", "wasm32-unknown-unknown", "--profile", "wasm"]);
run("wasm-bindgen", [
  "--target",
  "web",
  "--out-dir",
  out,
  "target/wasm32-unknown-unknown/wasm/fuwa_e2ee_wasm.wasm",
]);
