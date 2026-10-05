import { fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { ApplicationStatus, ServerSchema, type Server } from "@/gen/fuwa/v1/types_pb";
import { APPLIED } from "./account-keys";

/**
 * Servers you applied to and aren't in yet, so they wait in the rail with an
 * hourglass until someone lets you in. Kept per account in this browser; the
 * instance has the real answer, which the app asks for now and then.
 */
export type Applied = {
  server: Server;
  /** PENDING or REJECTED. */
  status: ApplicationStatus;
  /** Why it was turned down, when they said. */
  reason: string;
  appliedAt: number;
  /** The invite they applied with, to apply again with if it's turned down. */
  inviteCode: string;
};

/** `account` is "<instance>|<user id>": each account applies on its own. */
const KEY = (account: string) => APPLIED + account;

type Stored = { server: JsonValue; status: number; reason: string; appliedAt: number; inviteCode?: string };

export function loadApplied(account: string): Record<string, Applied> {
  try {
    const raw = localStorage.getItem(KEY(account));
    const list = raw ? (JSON.parse(raw) as Stored[]) : [];
    if (!Array.isArray(list)) return {};
    const out: Record<string, Applied> = {};
    for (const item of list) {
      const server = fromJson(ServerSchema, item.server, { ignoreUnknownFields: true });
      if (!server.id) continue;
      out[server.id] = {
        server,
        status: item.status === ApplicationStatus.REJECTED ? ApplicationStatus.REJECTED : ApplicationStatus.PENDING,
        reason: typeof item.reason === "string" ? item.reason : "",
        appliedAt: typeof item.appliedAt === "number" ? item.appliedAt : Date.now(),
        inviteCode: typeof item.inviteCode === "string" ? item.inviteCode : "",
      };
    }
    return out;
  } catch {
    return {};
  }
}

export function saveApplied(account: string, applied: Record<string, Applied>) {
  try {
    const list: Stored[] = Object.values(applied).map((a) => ({
      server: toJson(ServerSchema, a.server),
      status: a.status,
      reason: a.reason,
      appliedAt: a.appliedAt,
      inviteCode: a.inviteCode,
    }));
    if (list.length) localStorage.setItem(KEY(account), JSON.stringify(list));
    else localStorage.removeItem(KEY(account));
  } catch {
    // Private mode or storage full: the rail forgets them after a reload, nothing more.
  }
}
