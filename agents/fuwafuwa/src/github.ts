// Issues on GitHub. The token goes only to api.github.com, and never into a
// log or an error message.

export interface Issue {
  number: number;
  url: string;
}

export interface OpenIssue {
  number: number;
  title: string;
  body: string;
}

export interface GitHubOptions {
  /** "owner/name". */
  repo: string;
  /** A fine-grained token with Issues: read and write on `repo`, nothing else. */
  token: string;
  /** The label on the issues triage files, and the ones it adds feedback to. */
  label: string;
  fetch?: typeof fetch;
}

export class GitHub {
  readonly repo: string;
  readonly label: string;
  #token: string;
  #fetch: typeof fetch;

  constructor(options: GitHubOptions) {
    if (!/^[\w.-]+\/[\w.-]+$/.test(options.repo)) throw new Error("GITHUB_REPO must be owner/name");
    this.repo = options.repo;
    this.label = options.label;
    this.#token = options.token;
    this.#fetch = options.fetch ?? fetch;
  }

  async createIssue(title: string, body: string): Promise<Issue> {
    const issue = await this.#call("POST", "/issues", { title, body, labels: [this.label] });
    return { number: issue.number, url: issue.html_url };
  }

  /** Adds a comment; the link is the comment's. */
  async comment(number: number, body: string): Promise<Issue> {
    const comment = await this.#call("POST", `/issues/${number}/comments`, { body });
    return { number, url: comment.html_url };
  }

  /** Open issues with the label, newest first (up to 100). */
  async openIssues(): Promise<OpenIssue[]> {
    const q = new URLSearchParams({ state: "open", labels: this.label, per_page: "100" });
    const list = (await this.#call("GET", `/issues?${q}`)) as unknown as Array<{
      number: number;
      title: string;
      body?: string | null;
      pull_request?: unknown;
    }>;
    return list
      .filter((i) => !i.pull_request)
      .map((i) => ({ number: i.number, title: i.title, body: i.body ?? "" }));
  }

  async #call(method: string, path: string, body?: unknown): Promise<{ number: number; html_url: string }> {
    const res = await this.#fetch(`https://api.github.com/repos/${this.repo}${path}`, {
      method,
      headers: {
        accept: "application/vnd.github+json",
        authorization: `Bearer ${this.#token}`,
        "content-type": "application/json",
        "user-agent": "fuwafuwa",
        "x-github-api-version": "2022-11-28",
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(20_000),
    });
    if (!res.ok) throw new Error(`GitHub answered ${res.status} to ${method} ${path.split("?")[0]}`);
    return (await res.json()) as { number: number; html_url: string };
  }
}
