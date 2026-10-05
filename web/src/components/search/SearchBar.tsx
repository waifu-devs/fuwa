import { CalendarIcon, ClockIcon, HashIcon, PaperclipIcon, SearchIcon, UserIcon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { useFuwa } from "@/fuwa/store";
import { closeSearch, openSearch, runSearch, searchableChannel, searchPlace, useSearch } from "@/fuwa/search";
import { UserAvatar } from "@/components/Icons";
import { type I18n, type Key, useI18n } from "@/i18n/react";
import { fuzzy } from "@/lib/fuzzy";
import { comboLabel, bindingOf, actionById } from "@/lib/keybinds";
import { memberName } from "@/lib/format";
import { usePrefs } from "@/lib/prefs";
import {
  clearRecentSearches,
  FILTER_KEYS,
  HAS_VALUES,
  recentSearches,
  rememberSearch,
  replaceToken,
  tokenAt,
  type FilterKey,
} from "@/lib/search-query";
import { onCommand } from "@/lib/ui";
import { useMediaQuery } from "@/lib/use-media-query";
import { cn } from "@/lib/utils";
import type { Member } from "@/gen/fuwa/v1/types_pb";

/**
 * The search bar in a channel's header: Discord-style, with filters typed or
 * picked from suggestions (`from:`, `in:`, `has:`...), recent searches kept on
 * this device, and Ctrl+F (⌘F) to get to it from anywhere. On a phone it's a
 * button; the results panel then has the field.
 */

const EMPTY: never[] = [];
const ICON: Record<FilterKey, ReactNode> = {
  from: <UserIcon className="size-4" />,
  mentions: <UserIcon className="size-4" />,
  in: <HashIcon className="size-4" />,
  has: <PaperclipIcon className="size-4" />,
  before: <CalendarIcon className="size-4" />,
  during: <CalendarIcon className="size-4" />,
  after: <CalendarIcon className="size-4" />,
};

type Suggestion = {
  key: string;
  label: ReactNode;
  hint?: string;
  icon: ReactNode;
  /** Put in place of the word at the caret. */
  insert?: string;
  /** Searched right away. */
  search?: string;
  /** Typed key with a colon: pick a value next, without a space. */
  open?: boolean;
};

type Group = { id: string; title: string; items: Suggestion[]; clear?: () => void };

/** What each filter takes, in words. `has:`'s are its values, typed as they are, so they stay as written. */
const HINTS: Record<FilterKey, Key | null> = {
  from: "chattools.search.hint.member",
  in: "chattools.search.hint.channel",
  mentions: "chattools.search.hint.member",
  has: null,
  before: "chattools.search.hint.date",
  during: "chattools.search.hint.date",
  after: "chattools.search.hint.date",
};

const isoDay = (d: Date) =>
  `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;

function suggestionsFor(t: I18n["t"], token: string, input: string, members: Member[], channelNames: string[], recent: string[]): Group[] {
  const colon = token.indexOf(":");
  if (colon > 0) {
    const key = token.slice(0, colon).toLowerCase() as FilterKey;
    const value = token.slice(colon + 1).replace(/^[@#]/, "");
    if (!FILTER_KEYS.some((f) => f.key === key)) return [];
    if (key === "from" || key === "mentions") {
      const items = members
        .map((m) => {
          const name = m.user?.username ?? "";
          const score = Math.max(fuzzy(value, name)?.score ?? -1e9, fuzzy(value, memberName(m))?.score ?? -1e9);
          return { m, name, score };
        })
        .filter((x) => x.name && x.score > -1e9)
        .sort((a, b) => b.score - a.score)
        .slice(0, 6)
        .map(({ m, name }) => ({
          key: `${key}:${name}`,
          label: (
            <span className="flex min-w-0 items-center gap-2">
              <UserAvatar user={m.user} className="size-5 text-[0.6rem]" />
              <span className="truncate font-bold">{memberName(m)}</span>
              <span className="truncate text-muted-foreground">{name}</span>
            </span>
          ),
          icon: null,
          insert: `${key}:${name}`,
        }));
      return items.length ? [{ id: key, title: key === "from" ? t("chattools.search.group.from") : t("chattools.search.group.mentions"), items }] : [];
    }
    if (key === "in") {
      const items = channelNames
        .map((name) => ({ name, score: fuzzy(value, name)?.score }))
        .filter((x): x is { name: string; score: number } => x.score !== undefined)
        .sort((a, b) => b.score - a.score)
        .slice(0, 6)
        .map(({ name }) => ({ key: `in:${name}`, label: name, icon: <HashIcon className="size-4" />, insert: `in:${name}` }));
      return items.length ? [{ id: "in", title: t("chattools.search.group.in"), items }] : [];
    }
    if (key === "has") {
      const items = HAS_VALUES.filter((h) => h.aliases.some((a) => a.startsWith(value.toLowerCase()))).map((h) => ({
        key: `has:${h.value}`,
        label: h.label,
        icon: <PaperclipIcon className="size-4" />,
        insert: `has:${h.value}`,
      }));
      return items.length ? [{ id: "has", title: t("chattools.search.group.has"), items }] : [];
    }
    const now = new Date();
    const days = [
      { value: "today", hint: isoDay(now) },
      { value: "yesterday", hint: isoDay(new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1)) },
      { value: isoDay(new Date(now.getFullYear(), now.getMonth(), now.getDate() - 7)), hint: t("chattools.search.aWeekAgo") },
    ].filter((d) => d.value.startsWith(value.toLowerCase()));
    return days.length
      ? [{ id: "date", title: t("chattools.search.group.date"), items: days.map((d) => ({ key: `${key}:${d.value}`, label: d.value, hint: d.hint, icon: <CalendarIcon className="size-4" />, insert: `${key}:${d.value}` })) }]
      : [];
  }
  const groups: Group[] = [];
  const keys = FILTER_KEYS.filter((f) => !token || f.key.startsWith(token.toLowerCase())).map((f) => ({
    key: f.key,
    label: (
      <span>
        <span className="font-bold">{f.key}:</span> <span className="text-muted-foreground">{HINTS[f.key] ? t(HINTS[f.key]!) : f.hint}</span>
      </span>
    ),
    icon: ICON[f.key],
    insert: `${f.key}:`,
    open: true,
  }));
  if (keys.length && (token || !input.trim())) groups.push({ id: "options", title: t("chattools.search.group.options"), items: keys });
  if (!input.trim() && recent.length)
    groups.push({ id: "recent", title: t("chattools.search.group.recent"), items: recent.map((q) => ({ key: `recent:${q}`, label: q, icon: <ClockIcon className="size-4" />, search: q })) });
  return groups;
}

/** The text field with its suggestions. `inline` draws the suggestions in the flow (the phone panel) instead of floating. */
export function SearchField({
  instanceKey,
  serverId,
  inline = false,
  autoFocus = false,
  className,
}: {
  instanceKey: string;
  serverId: string;
  inline?: boolean;
  autoFocus?: boolean;
  className?: string;
}) {
  const shown = useSearch((s) => (s.open && s.instanceKey === instanceKey && s.serverId === serverId ? s.query : ""));
  const [input, setInput] = useState(shown);
  const [caret, setCaret] = useState(shown.length);
  const [focused, setFocused] = useState(false);
  const [active, setActive] = useState(-1);
  const [recentVersion, setRecentVersion] = useState(0);
  const field = useRef<HTMLInputElement>(null);
  const listId = useId();
  const members = useFuwa((s) => s.instances[instanceKey]?.members[serverId] ?? EMPTY);
  const channels = useFuwa((s) => s.instances[instanceKey]?.channels[serverId] ?? EMPTY);
  const channelNames = useMemo(() => channels.filter(searchableChannel).map((c) => c.name), [channels]);
  const meId = useFuwa((s) => s.instances[instanceKey]?.me?.id ?? "");
  const place = searchPlace(instanceKey, meId, serverId);
  const combo = usePrefs((p) => bindingOf(actionById("searchServer")!, p));
  const { t } = useI18n();

  // A search started elsewhere (a recent one, the phone's button) shows its words here.
  const [lastShown, setLastShown] = useState(shown);
  if (shown !== lastShown) {
    setLastShown(shown);
    if (shown) setInput(shown);
  }

  useEffect(() => onCommand("focusSearch", () => field.current?.focus()), []);

  // recentVersion re-reads the list after it changes.
  const recent = useMemo(() => (meId && recentVersion >= 0 ? recentSearches(place) : []), [meId, place, recentVersion]);
  const token = tokenAt(input, caret).text;
  const groups = useMemo(
    () => (focused ? suggestionsFor(t, token, input, members, channelNames, recent) : []),
    [t, focused, token, input, members, channelNames, recent],
  );
  const flat = groups.flatMap((g) => g.items);
  const open = focused && flat.length > 0;

  const search = (query: string) => {
    if (!query.trim()) return;
    void runSearch(instanceKey, serverId, query);
    setActive(-1);
    setRecentVersion((v) => v + 1);
  };

  const pick = (s: Suggestion) => {
    if (s.search !== undefined) {
      setInput(s.search);
      setCaret(s.search.length);
      search(s.search);
      field.current?.blur();
      return;
    }
    if (!s.insert) return;
    const next = s.open
      ? (() => {
          const { start, end } = tokenAt(input, caret);
          const value = `${input.slice(0, start)}${s.insert}${input.slice(end)}`;
          return { input: value, caret: start + s.insert.length };
        })()
      : replaceToken(input, caret, s.insert);
    setInput(next.input);
    setCaret(next.caret);
    setActive(-1);
    requestAnimationFrame(() => field.current?.setSelectionRange(next.caret, next.caret));
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown" && open) {
      e.preventDefault();
      setActive((a) => (a + 1) % flat.length);
    } else if (e.key === "ArrowUp" && open) {
      e.preventDefault();
      setActive((a) => (a <= 0 ? flat.length - 1 : a - 1));
    } else if ((e.key === "Tab" || e.key === "Enter") && open && active >= 0) {
      e.preventDefault();
      pick(flat[active]!);
    } else if (e.key === "Enter") {
      e.preventDefault();
      search(input);
      if (input.trim()) field.current?.blur();
    } else if (e.key === "Escape") {
      e.preventDefault();
      if (open) setFocused(false);
      else if (input) setInput("");
      else {
        closeSearch();
        field.current?.blur();
      }
    }
  };

  const clear = () => {
    setInput("");
    setCaret(0);
    closeSearch();
    field.current?.focus();
  };

  let row = -1;
  const list = (
    <AnimatePresence>
      {open && (
        <motion.div
          key="suggestions"
          id={listId}
          role="listbox"
          initial={{ opacity: 0, y: -6, scale: 0.97 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          exit={{ opacity: 0, y: -6, scale: 0.97, transition: { duration: 0.12 } }}
          transition={{ type: "spring", stiffness: 560, damping: 36 }}
          className={cn(
            "scroll-thin overflow-y-auto rounded-2xl border bg-popover p-1.5 text-popover-foreground shadow-xl",
            inline ? "mt-2 max-h-[50vh]" : "absolute top-full right-0 z-40 mt-2 max-h-96 w-80 origin-top-right",
          )}
          // Picking with the mouse mustn't take focus from the field first.
          onMouseDown={(e) => e.preventDefault()}
        >
          {groups.map((g) => (
            <div key={g.id} className="py-1">
              <div className="flex items-center justify-between px-2 pb-1 text-[0.7rem] font-extrabold tracking-wide text-muted-foreground uppercase">
                {g.title}
                {g.id === "recent" && (
                  <button
                    type="button"
                    className="rounded px-1 normal-case hover:text-foreground"
                    onClick={() => {
                      clearRecentSearches(place);
                      setRecentVersion((v) => v + 1);
                    }}
                  >
                    {t("chattools.search.clearRecent")}
                  </button>
                )}
              </div>
              {g.items.map((s) => {
                row++;
                const n = row;
                return (
                  <div key={s.key} className="group/suggestion relative">
                    <button
                      type="button"
                      role="option"
                      aria-selected={n === active}
                      onMouseEnter={() => setActive(n)}
                      onClick={() => pick(s)}
                      className={cn(
                        "flex w-full items-center gap-2 rounded-xl px-2 py-1.5 text-left text-sm transition-colors",
                        n === active ? "bg-primary/12 text-foreground" : "text-foreground/90",
                      )}
                    >
                      {s.icon && <span className="text-muted-foreground">{s.icon}</span>}
                      <span className="min-w-0 flex-1 truncate">{s.label}</span>
                      {s.hint && <span className="shrink-0 text-xs text-muted-foreground">{s.hint}</span>}
                    </button>
                    {s.search !== undefined && (
                      <button
                        type="button"
                        aria-label={t("chattools.search.forget")}
                        onClick={() => {
                          rememberSearch(place, s.search!, true);
                          setRecentVersion((v) => v + 1);
                        }}
                        className="absolute top-1/2 right-1.5 grid size-6 -translate-y-1/2 place-items-center rounded-full text-muted-foreground opacity-0 transition-opacity group-hover/suggestion:opacity-100 hover:bg-muted"
                      >
                        <XIcon className="size-3.5" />
                      </button>
                    )}
                  </div>
                );
              })}
            </div>
          ))}
        </motion.div>
      )}
    </AnimatePresence>
  );

  return (
    <div className={cn("relative", className)}>
      <div
        className={cn(
          "flex h-9 items-center gap-1.5 rounded-full border bg-background/60 px-3 transition-[border-color,box-shadow,background-color] duration-200",
          focused && "border-primary/50 bg-background ring-2 ring-primary/20",
        )}
      >
        <SearchIcon className={cn("size-4 shrink-0 transition-colors", focused ? "text-primary" : "text-muted-foreground")} />
        <input
          ref={field}
          value={input}
          autoFocus={autoFocus}
          onChange={(e) => {
            setInput(e.target.value);
            setCaret(e.target.selectionStart ?? e.target.value.length);
            setActive(-1);
          }}
          onSelect={(e) => setCaret(e.currentTarget.selectionStart ?? 0)}
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
          onKeyDown={onKeyDown}
          placeholder={t("chattools.search.placeholder")}
          aria-label={t("chattools.search.label")}
          role="combobox"
          aria-controls={listId}
          aria-autocomplete="list"
          aria-expanded={open}
          spellCheck={false}
          autoComplete="off"
          maxLength={200}
          className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
        />
        <AnimatePresence initial={false} mode="popLayout">
          {input ? (
            <motion.button
              key="clear"
              type="button"
              aria-label={t("chattools.search.clear")}
              initial={{ opacity: 0, scale: 0.5 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.5 }}
              onMouseDown={(e) => e.preventDefault()}
              onClick={clear}
              className="grid size-5 shrink-0 place-items-center rounded-full text-muted-foreground hover:bg-muted hover:text-foreground"
            >
              <XIcon className="size-3.5" />
            </motion.button>
          ) : (
            combo &&
            !focused && (
              <motion.kbd
                key="combo"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                className="shrink-0 rounded-md border bg-muted px-1.5 text-[0.65rem] font-bold text-muted-foreground"
              >
                {comboLabel(combo)}
              </motion.kbd>
            )
          )}
        </AnimatePresence>
      </div>
      {list}
    </div>
  );
}

/** The header's search: the field from tablets up, a button on phones (the panel has the field there). */
export function SearchBar({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const wide = useMediaQuery("(min-width: 640px)");
  const { t } = useI18n();
  useEffect(() => {
    if (wide) return;
    return onCommand("focusSearch", () => openSearch(instanceKey, serverId));
  }, [wide, instanceKey, serverId]);
  if (wide) return <SearchField instanceKey={instanceKey} serverId={serverId} className="w-44 lg:w-56" />;
  return (
    <motion.button
      type="button"
      aria-label={t("chattools.search.open")}
      onClick={() => openSearch(instanceKey, serverId)}
      whileTap={{ scale: 0.85 }}
      className="grid size-9 place-items-center rounded-full text-muted-foreground transition-colors hover:bg-muted"
    >
      <SearchIcon className="size-5" />
    </motion.button>
  );
}

