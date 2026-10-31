// boat.dev sandboxes, over its HTTP API (https://docs.boat.dev/api/v1). Only
// what building a roadmap card needs: make a sandbox, run a command in it,
// prompt the Claude Code that's signed in there, read what it did, download a
// file, stop it and delete it. The key goes only to boat.dev, and never into
// a log or an error message.

export type PromptStatus = "sending" | "queued" | "running" | "finished" | "failed" | "interrupted";

export interface Sandbox {
  id: string;
  /** provisioning, provisioned, cloning, ready, idle, running, archiving, archived or error. */
  state: string;
}

export interface CommandResult {
  exitCode: number;
  stdout: string;
  stderr: string;
}

export interface PromptRun {
  promptId: string;
  conversationId: string;
  status: PromptStatus;
  done: boolean;
}

export interface BoatEvent {
  id?: string;
  type: string;
  taskId?: string | null;
  conversationId?: string | null;
  data?: { content?: string; is_streaming?: boolean; is_reverted?: boolean; [k: string]: unknown };
}

export interface NewSandbox {
  /** small, default, large or xlarge. */
  type?: string;
  /** The boat environment (repositories, secrets, GitHub access, agent logins); boat's `base` when left out. */
  environment?: string;
  /** A saved snapshot to start from instead of a fresh machine. */
  from?: string;
  /** Seconds until boat stops it by itself; a running prompt pushes this out. */
  ttlSeconds: number;
  name?: string;
  labels?: Record<string, string>;
  /** Makes a retried create return the same sandbox instead of a second one. */
  idempotencyKey?: string;
}

/** An answer from boat that wasn't a success. `code` is boat's (provider_not_configured, rate_limited...). */
export class BoatError extends Error {
  readonly status: number;
  readonly code: string;
  readonly retryable: boolean;
  constructor(status: number, code: string, what: string, retryable: boolean) {
    super(`boat.dev answered ${status}${code ? ` (${code})` : ""} to ${what}`);
    this.name = "BoatError";
    this.status = status;
    this.code = code;
    this.retryable = retryable;
  }
}

export interface BoatOptions {
  /** A boat API key (boat_...). */
  apiKey: string;
  /** https://boat.dev/api/v1 unless set. */
  baseUrl?: string;
  fetch?: typeof fetch;
}

/** Ids boat makes: sandboxes are bx_ and eight letters or digits. */
export const SANDBOX_ID = /^bx_[23456789abcdefghjkmnpqrstuvwxyz]{8}$/;

export class Boat {
  #key: string;
  #base: string;
  #fetch: typeof fetch;

  constructor(options: BoatOptions) {
    if (!options.apiKey) throw new Error("BOAT_API_KEY is empty");
    this.#key = options.apiKey;
    this.#base = (options.baseUrl ?? "https://boat.dev/api/v1").replace(/\/+$/, "");
    this.#fetch = options.fetch ?? fetch;
  }

  async create(opts: NewSandbox): Promise<Sandbox> {
    const body: Record<string, unknown> = { ttlSeconds: opts.ttlSeconds };
    if (opts.type) body.type = opts.type;
    if (opts.environment) body.environment = opts.environment;
    if (opts.from) body.from = opts.from;
    if (opts.name) body.name = opts.name.slice(0, 120);
    if (opts.labels) body.labels = opts.labels;
    const headers: Record<string, string> = {};
    if (opts.idempotencyKey) headers["idempotency-key"] = opts.idempotencyKey;
    const res = await this.#json("POST", "/sandboxes", body, headers);
    return sandboxOf(res.sandbox);
  }

  async get(id: string): Promise<Sandbox> {
    const res = await this.#json("GET", `/sandboxes/${sandbox(id)}`);
    return sandboxOf(res.sandbox);
  }

  /** Waits until it takes commands and prompts (ready, idle or running). */
  async waitReady(id: string, timeoutMs = 10 * 60_000, sleep = delay): Promise<Sandbox> {
    const until = Date.now() + timeoutMs;
    for (;;) {
      const box = await this.get(id);
      if (box.state === "ready" || box.state === "idle" || box.state === "running") return box;
      if (box.state === "error" || box.state === "archived")
        throw new BoatError(409, `sandbox_${box.state}`, "waiting for the sandbox", false);
      if (Date.now() > until) throw new BoatError(408, "sandbox_slow", "waiting for the sandbox", true);
      await sleep(5_000);
    }
  }

  /** Runs a shell command and waits for it, up to `timeoutSeconds` (600 at most). */
  async run(id: string, command: string, timeoutSeconds = 120): Promise<CommandResult> {
    const res = await this.#json("POST", `/sandboxes/${sandbox(id)}/commands`, {
      command,
      timeoutSeconds: Math.min(600, timeoutSeconds),
    });
    return {
      exitCode: typeof res.exitCode === "number" ? res.exitCode : -1,
      stdout: typeof res.stdout === "string" ? res.stdout : "",
      stderr: typeof res.stderr === "string" ? res.stderr : "",
    };
  }

  /** Queues a task for Claude Code in a new conversation. */
  async prompt(id: string, prompt: string, options: { model?: string; reasoningEffort?: string } = {}): Promise<PromptRun> {
    const body: Record<string, unknown> = { provider: "claude-code", new: true, prompt };
    if (options.model) body.model = options.model;
    if (options.reasoningEffort) body.reasoningEffort = options.reasoningEffort;
    const res = await this.#json("POST", `/sandboxes/${sandbox(id)}/prompt`, body);
    return runOf(res.promptRun, res.conversationId);
  }

  async promptRun(id: string, promptId: string): Promise<PromptRun> {
    const res = await this.#json("GET", `/sandboxes/${sandbox(id)}/prompts/${encodeURIComponent(promptId)}`);
    return runOf(res.promptRun);
  }

  /** The newest of the agent's events in a conversation, newest first. */
  async events(id: string, conversationId: string, limit = 50): Promise<BoatEvent[]> {
    const q = new URLSearchParams({ sort: "desc", limit: String(limit) });
    if (conversationId) q.set("conversation", conversationId);
    const res = await this.#json("GET", `/sandboxes/${sandbox(id)}/events?${q}`);
    return Array.isArray(res.events) ? (res.events as BoatEvent[]) : [];
  }

  /** Stops the agent's turn in that conversation. */
  async interrupt(id: string, conversationId: string): Promise<void> {
    const q = conversationId ? `?${new URLSearchParams({ conversation: conversationId })}` : "";
    await this.#json("POST", `/sandboxes/${sandbox(id)}/interrupt${q}`, {});
  }

  /** A file under /home/user or /tmp, up to 50 MiB. */
  async download(id: string, path: string): Promise<Uint8Array> {
    const res = await this.#send("GET", `/sandboxes/${sandbox(id)}/artifacts?${new URLSearchParams({ path })}`);
    return new Uint8Array(await res.arrayBuffer());
  }

  /** Stops it and keeps its disk (14 days), so it can be resumed. One already stopped is fine. */
  async stop(id: string): Promise<void> {
    try {
      await this.#json("POST", `/sandboxes/${sandbox(id)}/stop`, {});
    } catch (err) {
      if (!(err instanceof BoatError && (err.status === 404 || err.status === 409))) throw err;
    }
  }

  /** Deletes it and its snapshots for good. One already gone is fine. */
  async delete(id: string): Promise<void> {
    try {
      await this.#json("DELETE", `/sandboxes/${sandbox(id)}`, undefined, { "x-ascii-confirm-delete": id });
    } catch (err) {
      if (!(err instanceof BoatError && err.status === 404)) throw err;
    }
  }

  async #json(method: string, path: string, body?: unknown, headers: Record<string, string> = {}): Promise<Record<string, unknown>> {
    const res = await this.#send(method, path, body, headers);
    return (await res.json().catch(() => ({}))) as Record<string, unknown>;
  }

  async #send(method: string, path: string, body?: unknown, headers: Record<string, string> = {}): Promise<Response> {
    // The path without its query: what's said in errors.
    const what = `${method} ${path.split("?")[0]!.replace(/bx_\w+/, "{sandbox}")}`;
    for (let attempt = 0; ; attempt++) {
      const res = await this.#fetch(this.#base + path, {
        method,
        headers: {
          accept: "application/json",
          authorization: `Bearer ${this.#key}`,
          ...(body === undefined ? {} : { "content-type": "application/json" }),
          "user-agent": "fuwafuwa",
          ...headers,
        },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(method === "POST" && path.endsWith("/commands") ? 660_000 : 60_000),
      });
      if (res.ok) return res;
      const err = (await res.json().catch(() => ({}))) as {
        code?: string;
        error?: { code?: string; details?: { retryable?: boolean; retryAfterSeconds?: number } };
        retryAfterSeconds?: number;
      };
      const code = err.code ?? err.error?.code ?? "";
      const retryable =
        res.status === 503 ||
        code === "sandbox_starting" ||
        code === "idempotency_in_progress" ||
        (res.status === 502 && err.error?.details?.retryable === true);
      if (retryable && attempt < 4) {
        const after = err.retryAfterSeconds ?? err.error?.details?.retryAfterSeconds;
        await delay(typeof after === "number" && after > 0 ? after * 1000 : 2000 * 2 ** attempt);
        continue;
      }
      throw new BoatError(res.status, code, what, retryable);
    }
  }
}

function sandbox(id: string): string {
  if (!SANDBOX_ID.test(id)) throw new Error("not a boat sandbox id");
  return id;
}

function sandboxOf(value: unknown): Sandbox {
  const s = (value ?? {}) as { id?: unknown; state?: unknown };
  if (typeof s.id !== "string") throw new BoatError(200, "no_sandbox", "reading the sandbox", false);
  return { id: s.id, state: typeof s.state === "string" ? s.state : "" };
}

function runOf(value: unknown, conversationId?: unknown): PromptRun {
  const r = (value ?? {}) as { promptId?: unknown; id?: unknown; conversationId?: unknown; status?: unknown; done?: unknown };
  const promptId = typeof r.promptId === "string" ? r.promptId : typeof r.id === "string" ? r.id : "";
  if (!promptId) throw new BoatError(200, "no_prompt", "reading the prompt", false);
  const convo = typeof r.conversationId === "string" ? r.conversationId : typeof conversationId === "string" ? conversationId : "";
  return {
    promptId,
    conversationId: convo,
    status: (typeof r.status === "string" ? r.status : "queued") as PromptStatus,
    done: r.done === true,
  };
}

function delay(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}
