import { AccountKind, type User } from "@/gen/fuwa/v1/types_pb";

/** Accounts made on the instance sign in with a password; linked ones sign in through waifu.dev, and agents with a token. */
export const hasPassword = (user: User | undefined) => !!user && user.kind === AccountKind.LOCAL;
