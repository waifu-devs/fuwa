import { fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { ApplicationStatus, ServerSchema, type Server } from "@/gen/fuwa/v1/types_pb";

/**
 * Servers you applied to and aren't in yet, so they wait in the rail with an
 * hourglass until someone lets you in. Kept per instance in this browser; the
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

const KEY = (instanceKey: string) => `fuwa:applied:${instanceKey}`;

type Stored = { server: JsonValue; status: number; reason: string; appliedAt: number; inviteCode?: string };

export function loadApplied(instanceKey: string): Record<string, Applied> {
  try {
    const raw = localStorage.getItem(KEY(instanceKey));
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

export function saveApplied(instanceKey: string, applied: Record<string, Applied>) {
  try {
    const list: Stored[] = Object.values(applied).map((a) => ({
      server: toJson(ServerSchema, a.server),
      status: a.status,
      reason: a.reason,
      appliedAt: a.appliedAt,
      inviteCode: a.inviteCode,
    }));
    if (list.length) localStorage.setItem(KEY(instanceKey), JSON.stringify(list));
    else localStorage.removeItem(KEY(instanceKey));
  } catch {
    // Private mode or storage full: the rail forgets them after a reload, nothing more.
  }
}
