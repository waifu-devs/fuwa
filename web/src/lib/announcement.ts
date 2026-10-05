import { MegaphoneIcon, SirenIcon, TriangleAlertIcon } from "lucide-react";
import { AnnouncementTone, type Announcement } from "@/gen/fuwa/v1/types_pb";
import { formatStamp, formatTime, sameDay, toDate } from "@/lib/format";

/** The instance's announcement: whether it's up, its tone, and when it comes down. */

/** Whether an announcement is up now: set, with text, and not past its end. */
export function isLive(a: Announcement | undefined, now = Date.now()): a is Announcement {
  return !!a?.text && (!a.endsAt || toDate(a.endsAt).getTime() > now);
}

export const TONES = {
  [AnnouncementTone.INFO]: { icon: MegaphoneIcon, className: "announcement-info" },
  [AnnouncementTone.WARNING]: { icon: TriangleAlertIcon, className: "announcement-warning" },
  [AnnouncementTone.CRITICAL]: { icon: SirenIcon, className: "announcement-critical" },
} as const;

export const toneOf = (a: Pick<Announcement, "tone">) =>
  a.tone === AnnouncementTone.WARNING || a.tone === AnnouncementTone.CRITICAL ? a.tone : AnnouncementTone.INFO;

/** When it comes down: a time today, or a day and time. */
export const endsLabel = (d: Date) => (sameDay(d, new Date()) ? formatTime(d) : formatStamp(d));
