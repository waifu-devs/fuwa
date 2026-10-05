import { CalendarHeartIcon, CrownIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import type { ReactNode } from "react";
import type { Member, Profile, User } from "@/gen/fuwa/v1/types_pb";
import { hue, UserAvatar } from "@/components/Icons";
import { Markdown } from "@/components/Markdown";
import { SwapText } from "@/components/motion";
import { SPRING } from "@/lib/motion";
import { Private } from "@/components/Private";
import { AppBadge } from "@/components/AppBadge";
import { ActivityCards, PresenceDot } from "@/components/Presence";
import { ProfileEffect } from "@/components/ProfileEffect";
import { colorCss, displayName, hueOf, isAgent, shownStatus, toDate } from "@/lib/format";
import { useI18n } from "@/i18n/react";
import { usePrefs } from "@/lib/prefs";
import { shownPicture } from "@/lib/shown";
import { cn } from "@/lib/utils";

const stagger = (n: number) => ({ ...SPRING, delay: 0.06 + n * 0.04 });

const PlainText = ({ children, className }: { children: string; className?: string }) => <span className={cn("inline-block max-w-full", className)}>{children}</span>;

/**
 * Someone's profile as others see it: their banner (a picture, or their
 * color), avatar, name, pronouns, status and bio, and how long they've been
 * around. The same card opens from the member list and messages, and shows
 * your own while you edit it. Their profile effect, if they picked one, plays
 * over it (others' only while the viewer lets them).
 */
export function ProfileCard({
  user,
  profile,
  member,
  owner = false,
  roles,
  me = false,
  loading = false,
  editing = false,
  instanceKey,
  className,
}: {
  user: User;
  /** The rest of the profile; while it loads, the card shows what it has. */
  profile?: Pick<Profile, "pronouns" | "bio" | "bannerUrl" | "accentColor" | "createdAt"> & Partial<Pick<Profile, "effect">>;
  /** Their membership in the server it opened from. */
  member?: Member;
  /** They own the server it opened from. */
  owner?: boolean;
  /** Their roles in that server. */
  roles?: ReactNode;
  /** Your own card: your username hides in streamer mode. */
  me?: boolean;
  loading?: boolean;
  /** A live preview while you type: text changes in place instead of sliding. */
  editing?: boolean;
  /** Where they're seen, for their presence: their dot and what they're doing. */
  instanceKey?: string;
  className?: string;
}) {
  const Text = editing ? PlainText : SwapText;
  const name = member?.nickname || displayName(user);
  const status = shownStatus(user);
  const accent = profile?.accentColor;
  const since = profile?.createdAt ? toDate(profile.createdAt) : null;
  const joined = member?.joinedAt ? toDate(member.joinedAt) : null;
  const othersEffects = usePrefs((p) => p.othersEffects);
  const effect = me || othersEffects ? profile?.effect : undefined;

  return (
    <div className={cn("relative overflow-hidden rounded-3xl border bg-card shadow-xl", className)}>
      {effect && <ProfileEffect key={effect} effect={effect} seed={user.id} color={accent === undefined ? `hsl(${hueOf(user.id)} 85% 72%)` : colorCss(accent)} />}
      <ProfileBanner userId={user.id} bannerUrl={profile?.bannerUrl} accent={accent} />
      <div className="relative px-4 pb-4">
        <div className="-mt-11 flex items-end gap-2">
          <motion.span
            initial={{ scale: 0.6, rotate: -12, opacity: 0 }}
            animate={{ scale: 1, rotate: 0, opacity: 1 }}
            transition={{ type: "spring", stiffness: 420, damping: 18 }}
            className="avatar-ring inline-block shrink-0 rounded-full p-[3px]"
          >
            <span className="relative block">
              <UserAvatar user={user} className="size-20 text-3xl ring-4 ring-card" />
              {instanceKey && <PresenceDot instanceKey={instanceKey} userId={user.id} className="absolute right-0.5 bottom-0.5 size-5 ring-[5px]" />}
            </span>
          </motion.span>
          <AnimatePresence>
            {status && (
              <motion.p
                key="status"
                initial={{ opacity: 0, scale: 0.6, x: -12, originX: 0, originY: 1 }}
                animate={{ opacity: 1, scale: 1, x: 0 }}
                exit={{ opacity: 0, scale: 0.6 }}
                transition={{ type: "spring", stiffness: 500, damping: 22, delay: 0.15 }}
                className="relative mb-9 min-w-0 rounded-2xl rounded-bl-md border bg-popover px-3 py-1.5 text-sm shadow-md"
              >
                <span className="line-clamp-2 break-words">
                  <Text>{status}</Text>
                </span>
              </motion.p>
            )}
          </AnimatePresence>
        </div>
        <NameLines user={user} name={name} member={member} owner={owner} me={me} pronouns={profile?.pronouns} Text={Text} />
        {roles}
        {instanceKey && <ActivityCards instanceKey={instanceKey} userId={user.id} />}
        <AnimatePresence initial={false}>
          {(profile?.bio || loading) && (
            <motion.div
              key="bio"
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: "auto" }}
              exit={{ opacity: 0, height: 0 }}
              transition={stagger(1)}
              className="overflow-hidden"
            >
              <AboutMe bio={profile?.bio} />
            </motion.div>
          )}
        </AnimatePresence>
        <Dates since={since} joined={joined} />
      </div>
    </div>
  );
}

/** Their banner: their picture, or their color (or one picked from their id) shaded toward the bottom. */
function ProfileBanner({ userId, bannerUrl, accent }: { userId: string; bannerUrl?: string; accent?: number }) {
  return (
    <div className="relative h-28 overflow-hidden">
      <AnimatePresence initial={false}>
        <motion.div
          key={bannerUrl ? `picture-${bannerUrl}` : accent === undefined ? "hue" : "color"}
          initial={{ opacity: 0, scale: 1.12 }}
          animate={{ opacity: 1, scale: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.6, ease: [0.22, 1, 0.36, 1] }}
          style={accent === undefined ? hue(userId) : { backgroundColor: colorCss(accent) }}
          className={cn("absolute inset-0 transition-colors duration-500", accent === undefined && "server-gradient")}
        >
          {accent !== undefined && <span className="absolute inset-0 bg-gradient-to-br from-transparent via-transparent to-black/45" />}
          {shownPicture(bannerUrl) && <img src={shownPicture(bannerUrl)} alt="" className="relative size-full object-cover" draggable={false} />}
        </motion.div>
      </AnimatePresence>
    </div>
  );
}

/** Their name with an owner crown and app badge, then their username, pronouns and display name under a nickname. */
function NameLines({
  user,
  name,
  member,
  owner,
  me,
  pronouns,
  Text,
}: {
  user: User;
  name: string;
  member?: Member;
  owner: boolean;
  me: boolean;
  pronouns?: string;
  Text: typeof SwapText;
}) {
  const { t } = useI18n();
  return (
    <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={stagger(0)} className="mt-2">
      <p className="flex min-w-0 items-center gap-1.5 text-xl font-extrabold">
        <Text className="truncate">{name}</Text>
        {owner && <CrownIcon aria-label={t("workspace.profile.owner")} className="size-4 shrink-0 text-amber-400" />}
        {isAgent(user) && <AppBadge agent />}
      </p>
      <p className="flex min-w-0 flex-wrap items-center gap-x-1.5 text-sm text-muted-foreground">
        <span className="truncate">@{me ? <Private text={user.username} kind="name" /> : user.username}</span>
        <AnimatePresence>
          {pronouns && (
            <motion.span
              key="pronouns"
              initial={{ opacity: 0, scale: 0.8 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.8 }}
              transition={SPRING}
              className="rounded-full bg-muted px-2 py-0.5 text-xs font-bold text-foreground/80"
            >
              {pronouns}
            </motion.span>
          )}
        </AnimatePresence>
        {member?.nickname && member.nickname !== displayName(user) && <span className="truncate">· {displayName(user)}</span>}
      </p>
    </motion.div>
  );
}

/** Their bio, or a shimmer while it loads. */
function AboutMe({ bio }: { bio?: string }) {
  const { t } = useI18n();
  return (
    <div className="mt-3 rounded-2xl bg-muted/60 p-3">
      <p className="mb-1 text-[0.7rem] font-extrabold tracking-wide text-muted-foreground uppercase">{t("workspace.profile.aboutMe")}</p>
      {bio ? (
        <Markdown className="max-h-48 overflow-y-auto text-sm">{bio}</Markdown>
      ) : (
        <div className="flex flex-col gap-1.5 py-1">
          <span className="shimmer h-3 w-11/12 rounded" />
          <span className="shimmer h-3 w-2/3 rounded" />
        </div>
      )}
    </div>
  );
}

/** How long they've been around, and in this server. */
function Dates({ since, joined }: { since: Date | null; joined: Date | null }) {
  const { t, date } = useI18n();
  if (!since && !joined) return null;
  const day = (d: Date) => date(d, { year: "numeric", month: "short", day: "numeric" });
  return (
    <motion.div
      initial={{ opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={stagger(2)}
      className="mt-3 flex flex-col gap-1 text-xs text-muted-foreground"
    >
      {since && (
        <span className="flex items-center gap-1.5">
          <CalendarHeartIcon className="size-3.5" /> {t("workspace.profile.since", { date: day(since) })}
        </span>
      )}
      {joined && <span className="pl-5">{t("workspace.profile.joined", { date: day(joined) })}</span>}
    </motion.div>
  );
}
