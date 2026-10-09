// Runs a real fuwa instance for the tests: the binary at $FUWA_BIN (built
// with `cargo build -p fuwa-server --bin fuwa`), on a free port, with its
// data in a temporary folder.
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";

export interface Instance {
  url: string;
  /** Stops the process (the data stays) and starts it again on the same port. */
  restart(): Promise<void>;
  stop(): Promise<void>;
}

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const srv = createServer();
    srv.unref();
    srv.on("error", reject);
    srv.listen(0, "127.0.0.1", () => {
      const { port } = srv.address() as { port: number };
      srv.close(() => resolve(port));
    });
  });
}

export async function startInstance(): Promise<Instance> {
  const bin = process.env.FUWA_BIN;
  if (!bin) throw new Error("set FUWA_BIN to a fuwa binary (cargo build -p fuwa-server --bin fuwa)");
  const port = await freePort();
  const data = mkdtempSync(join(tmpdir(), "fuwa-sdk-"));
  const url = `http://127.0.0.1:${port}`;
  let child: ChildProcess | undefined;
  let output = "";

  const launch = async () => {
    output = "";
    child = spawn(bin, [], {
      env: {
        ...process.env,
        FUWA_DATA_PATH: data,
        FUWA_HOST: "127.0.0.1",
        FUWA_PORT: String(port),
        FUWA_PUBLIC_URL: url,
        FUWA_TELEMETRY: "off",
        FUWA_WEB: "off",
        // Calls on, on a port of the system's choosing: the voice bridge needs the media part.
        FUWA_MEDIA_PORT: "0",
        FUWA_MEDIA_ADDRESSES: "127.0.0.1",
        // Agents' endpoints may be on this computer, where the tests run them.
        FUWA_AGENT_ENDPOINTS: "any",
        RUST_LOG: "warn",
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    child.stdout?.on("data", (b) => (output += b));
    child.stderr?.on("data", (b) => (output += b));
    for (let i = 0; i < 300; i++) {
      if (child.exitCode !== null) throw new Error(`fuwa exited (${child.exitCode}):\n${output}`);
      try {
        if ((await fetch(`${url}/healthz`)).ok) return;
      } catch {
        // not listening yet
      }
      await new Promise((r) => setTimeout(r, 100));
    }
    throw new Error(`fuwa didn't start:\n${output}`);
  };
  const halt = async () => {
    const c = child;
    if (!c || c.exitCode !== null) return;
    const exited = new Promise((r) => c.once("exit", r));
    c.kill("SIGTERM");
    const timer = setTimeout(() => c.kill("SIGKILL"), 5000);
    await exited;
    clearTimeout(timer);
  };

  await launch();
  return {
    url,
    async restart() {
      await halt();
      await launch();
    },
    async stop() {
      await halt();
      rmSync(data, { recursive: true, force: true });
    },
  };
}

/** Polls `check` until it returns something, or fails after `ms`. */
export async function until<T>(what: string, check: () => T | undefined | Promise<T | undefined>, ms = 10_000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = await check();
    if (v !== undefined && v !== false) return v as T;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 50));
  }
}
