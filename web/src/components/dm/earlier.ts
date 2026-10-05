import { isMessage, type Item } from "@/e2ee/vault";

/** Where the messages from before this device joined came from: passed on by a member, or this account's backup. */
/** Or, with none here, "restorable" if the account's backup could bring them. */
export type Earlier = "shared" | "backup" | "restorable" | null;

export function earlierFrom(items: Item[] | undefined, locked: boolean): Earlier {
  if (items?.some((i) => i.sharedBy)) return "shared";
  const joined = items?.findLast((i) => i.kind === "joined");
  if (joined && items!.some((i) => isMessage(i) && i.seq < joined.seq)) return "backup";
  return locked ? "restorable" : null;
}
