import type { ReactNode } from "react";
import { type Key, T } from "@/i18n/react";
import { hueOf } from "@/lib/format";

/** Ways to say someone joined, picked by who they are so each join keeps its line. */
const JOIN_LINES: Key[] = [
  "chat.join.line1",
  "chat.join.line2",
  "chat.join.line3",
  "chat.join.line4",
  "chat.join.line5",
  "chat.join.line6",
  "chat.join.line7",
  "chat.join.line8",
];

export const joinLine = (userId: string, name: ReactNode) => <T k={JOIN_LINES[hueOf(userId) % JOIN_LINES.length]!} values={{ name }} />;
