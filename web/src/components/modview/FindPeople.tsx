import { BanIcon, LoaderCircleIcon, SearchIcon, SearchXIcon, UserRoundSearchIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useState } from "react";
import type { ModPerson } from "@/gen/fuwa/v1/moderation_pb";
import { findPeople } from "@/fuwa/moderation";
import { UserAvatar } from "@/components/Icons";
import { Empty, Handle, useScope } from "@/components/modview/shared";
import { Input } from "@/components/ui/input";
import { useI18n } from "@/i18n/react";
import { displayName } from "@/lib/format";
import { SPRING } from "@/lib/motion";
import { cn } from "@/lib/utils";

/** How long typing rests before the instance is asked. */
const REST_MS = 200;

/**
 * Finding someone to look into: everyone the server knows, members first,
 * and people who left or were banned too, which the member list can't reach.
 */
export function FindPeople() {
  const { t } = useI18n();
  const { instanceKey, server, meId, learn, openPerson } = useScope();
  const [query, setQuery] = useState("");
  const [found, setFound] = useState<{ query: string; people: ModPerson[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const asked = query.trim().replace(/^@/, "").trim();

  useEffect(() => {
    if (!asked) return;
    const abort = new AbortController();
    const timer = setTimeout(() => {
      findPeople(instanceKey, server.id, asked, abort.signal)
        .then((res) => {
          learn(res.people.flatMap((p) => (p.user ? [p.user] : [])));
          setFound({ query: asked, people: res.people });
          setError(null);
        })
        .catch((err: Error) => !abort.signal.aborted && setError(err.message));
    }, REST_MS);
    return () => {
      clearTimeout(timer);
      abort.abort();
    };
  }, [instanceKey, server.id, asked, learn]);

  const shown = asked && found ? found.people : [];
  const waiting = !!asked && found?.query !== asked && !error;

  return (
    <div className="flex max-w-2xl flex-col gap-3">
      <div className="relative">
        <SearchIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          maxLength={100}
          placeholder={t("chattools.modview.findPlaceholder")}
          aria-label={t("chattools.modview.find")}
          className="h-11 rounded-xl pr-10 pl-9"
        />
        <AnimatePresence>
          {waiting && (
            <motion.span initial={{ opacity: 0, scale: 0.6 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.6 }} className="absolute top-1/2 right-3 -translate-y-1/2">
              <LoaderCircleIcon className="size-4 animate-spin text-muted-foreground" />
            </motion.span>
          )}
        </AnimatePresence>
      </div>
      {error && <p className="text-sm text-destructive first-letter:uppercase">{error}</p>}
      {!asked && <Empty icon={UserRoundSearchIcon} title={t("chattools.modview.findStart")} hint={t("chattools.modview.findHint")} />}
      {asked && found && !waiting && !shown.length && <Empty icon={SearchXIcon} title={t("chattools.modview.findNone")} />}
      {shown.length > 0 && (
        <ol className={cn("flex flex-col gap-1.5 transition-opacity", waiting && "opacity-60")}>
          <AnimatePresence initial={false} mode="popLayout">
            {shown.map((person, i) => (
              <PersonRow key={person.user?.id ?? i} person={person} index={i} me={person.user?.id === meId} onOpen={() => person.user && openPerson(person.user.id)} />
            ))}
          </AnimatePresence>
        </ol>
      )}
    </div>
  );
}

/** One person found: who they are here, and whether they're still in or banned. */
function PersonRow({ person, index, me, onOpen }: { person: ModPerson; index: number; me: boolean; onOpen: () => void }) {
  const { t } = useI18n();
  const user = person.user;
  const name = person.nickname || displayName(user);
  return (
    <motion.li
      layout="position"
      initial={{ opacity: 0, x: -12 }}
      animate={{ opacity: 1, x: 0, transition: { ...SPRING, delay: Math.min(index, 14) * 0.025 } }}
      exit={{ opacity: 0, scale: 0.96, transition: { duration: 0.15 } }}
    >
      <button
        type="button"
        onClick={onOpen}
        className="group flex w-full items-center gap-3 rounded-2xl border bg-background/40 p-3 text-left transition-colors hover:border-primary/30 hover:bg-muted/30 active:scale-[0.99]"
      >
        <UserAvatar user={user} className={cn("size-9 shrink-0 transition-transform duration-300 group-hover:scale-110", !person.member && "opacity-60 grayscale")} />
        <span className="min-w-0 flex-1">
          <b className="block truncate text-sm">{name}</b>
          <span className="flex min-w-0 items-center gap-1 text-xs text-muted-foreground">
            <Handle user={user} me={me} />
            {person.nickname && person.nickname !== displayName(user) && <span className="truncate">· {displayName(user)}</span>}
          </span>
        </span>
        {person.banned ? (
          <span className="flex shrink-0 items-center gap-1 rounded-full bg-destructive/10 px-2 py-0.5 text-[0.7rem] font-extrabold text-destructive">
            <BanIcon className="size-3" /> {t("chattools.modview.bannedTag")}
          </span>
        ) : (
          !person.member && <span className="shrink-0 rounded-full bg-muted px-2 py-0.5 text-[0.7rem] font-extrabold text-muted-foreground">{t("chattools.modview.notMember")}</span>
        )}
      </button>
    </motion.li>
  );
}
