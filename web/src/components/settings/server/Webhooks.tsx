import {
  CheckIcon,
  ChevronDownIcon,
  CopyIcon,
  EyeIcon,
  EyeOffIcon,
  HashIcon,
  LoaderCircleIcon,
  MegaphoneIcon,
  PlusIcon,
  RefreshCwIcon,
  SendIcon,
  TerminalIcon,
  Trash2Icon,
  WebhookIcon,
  XIcon,
} from "lucide-react";
import { AnimatePresence, motion, useAnimationControls } from "motion/react";
import { useEffect, useMemo, useState } from "react";
import { ChannelType, type Channel, type User, type Webhook } from "@/gen/fuwa/v1/types_pb";
import { createWebhook, deleteWebhook, listWebhooks, resetWebhookToken, run, testWebhook, updateWebhook, webhookUrl } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useInstance } from "@/fuwa/hooks";
import { UserAvatar } from "@/components/Icons";
import { PictureField } from "@/components/PictureField";
import { Count, SPRING } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { ago, displayName, toDate } from "@/lib/format";
import { T, useI18n } from "@/i18n/react";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** Names new webhooks get, to be changed. */
const NAMES = ["Captain Hook", "Fuwa Hook", "Hooky", "Post Bot", "Little Messenger", "Paper Plane"];

/** What a webhook looks like where people see it: its name and picture, as a profile. */
const asUser = (w: Pick<Webhook, "id" | "name" | "avatarUrl">) => ({ id: w.id, displayName: w.name, username: w.name, avatarUrl: w.avatarUrl }) as User;

/**
 * The server's webhooks: addresses other apps (CI, feeds, alerts, anything
 * made for Discord webhooks) post messages to, each into one channel.
 */
export function Webhooks({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const channels = useMemo(
    () => (inst?.channels[serverId] ?? []).filter((c) => c.type === ChannelType.TEXT || c.type === ChannelType.ANNOUNCEMENT),
    [inst?.channels, serverId],
  );
  const [webhooks, setWebhooks] = useState<Webhook[] | null>(null);
  const [creators, setCreators] = useState<Record<string, User>>({});
  const [open, setOpen] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [howTo, setHowTo] = useState(false);

  useEffect(() => {
    run(listWebhooks(instanceKey, serverId)).then(
      (r) => {
        setWebhooks(r.webhooks);
        setCreators(Object.fromEntries(r.creators.map((u) => [u.id, u])));
      },
      (err: FuwaError) => {
        toast(err.message);
        setWebhooks([]);
      },
    );
  }, [instanceKey, serverId]);

  const replace = (w: Webhook) => setWebhooks((list) => list?.map((x) => (x.id === w.id ? w : x)) ?? null);

  async function add() {
    const channel = channels[0];
    if (!channel) return toast(t("serversettings.webhooks.needChannel"));
    setCreating(true);
    const name = NAMES[(webhooks?.length ?? 0) % NAMES.length]!;
    try {
      const w = await run(createWebhook(instanceKey, serverId, channel.id, name));
      setWebhooks((list) => [...(list ?? []), w]);
      if (inst?.me) setCreators((c) => ({ ...c, [inst.me!.id]: inst.me! }));
      setOpen(w.id);
    } catch (err) {
      toast((err as FuwaError).message);
    }
    setCreating(false);
  }

  const example = webhooks?.[0];
  const exampleUrl = example && inst ? webhookUrl(inst.url, example) : `${inst?.url ?? "https://fuwa.example"}/webhooks/…`;

  return (
    <div className="flex flex-col gap-5">
      <div className="relative overflow-hidden rounded-3xl border bg-gradient-to-br from-primary/10 via-transparent to-transparent p-5">
        <Wires />
        <div className="relative flex flex-wrap items-center gap-4">
          <motion.span
            animate={{ rotate: [0, -8, 8, 0], y: [0, -3, 0] }}
            transition={{ duration: 3, repeat: Infinity, repeatDelay: 2 }}
            className="grid size-12 shrink-0 place-items-center rounded-2xl bg-primary/15 text-primary"
          >
            <WebhookIcon className="size-6" />
          </motion.span>
          <div className="min-w-0 flex-1 basis-60">
            <p className="font-extrabold">{t("serversettings.webhooks.title")}</p>
            <p className="text-sm text-muted-foreground">{t("serversettings.webhooks.intro")}</p>
          </div>
          <Button type="button" className="btn rounded-xl font-bold" disabled={creating || !webhooks} onClick={() => void add()}>
            {creating ? <LoaderCircleIcon className="animate-spin" /> : <PlusIcon />} {t("serversettings.webhooks.new")}
          </Button>
        </div>
      </div>

      {!webhooks ? (
        <div className="flex flex-col gap-2">
          {[0, 1].map((n) => (
            <div key={n} className="shimmer h-16 rounded-2xl" />
          ))}
        </div>
      ) : webhooks.length === 0 ? (
        <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} className="flex flex-col items-center gap-2 py-8 text-center">
          <motion.span animate={{ y: [0, -6, 0], rotate: [0, 6, -6, 0] }} transition={{ duration: 2.6, repeat: Infinity, repeatDelay: 0.8 }} className="text-4xl">
            🪝
          </motion.span>
          <p className="font-bold">{t("serversettings.webhooks.none")}</p>
          <p className="text-sm text-muted-foreground">{t("serversettings.webhooks.noneHint")}</p>
        </motion.div>
      ) : (
        <ul className="flex flex-col gap-2">
          <AnimatePresence initial={false}>
            {webhooks.map((w) => (
              <WebhookCard
                key={w.id}
                instanceKey={instanceKey}
                url={inst ? webhookUrl(inst.url, w) : ""}
                webhook={w}
                channels={channels}
                creator={creators[w.creatorId]}
                open={open === w.id}
                onToggle={() => setOpen((o) => (o === w.id ? null : w.id))}
                onChange={replace}
                onDelete={() => setWebhooks((list) => list?.filter((x) => x.id !== w.id) ?? null)}
              />
            ))}
          </AnimatePresence>
        </ul>
      )}

      <div className="rounded-2xl border">
        <button type="button" onClick={() => setHowTo((h) => !h)} className="flex w-full items-center gap-2 p-3 text-left text-sm font-bold">
          <TerminalIcon className="size-4 text-primary" />
          <span className="flex-1">{t("serversettings.webhooks.howTo")}</span>
          <ChevronDownIcon className={cn("size-4 text-muted-foreground transition-transform duration-300", howTo && "rotate-180")} />
        </button>
        <AnimatePresence initial={false}>
          {howTo && (
            <motion.div
              initial={{ height: 0, opacity: 0 }}
              animate={{ height: "auto", opacity: 1 }}
              exit={{ height: 0, opacity: 0 }}
              transition={SPRING}
              className="overflow-hidden"
            >
              <div className="flex flex-col gap-2 px-3 pb-3 text-sm text-muted-foreground">
                <p>
                  <T
                    k="serversettings.webhooks.howToPost"
                    values={{
                      post: <b>POST</b>,
                      content: <code>content</code>,
                      username: <code>username</code>,
                      avatarUrl: <code>avatar_url</code>,
                      embeds: <code>embeds</code>,
                      wait: <code>?wait=true</code>,
                    }}
                  />
                </p>
                <pre className="overflow-x-auto rounded-xl bg-muted p-3 font-mono text-xs leading-relaxed text-foreground">
                  {`curl -X POST '${exampleUrl}' \\\n  -H 'content-type: application/json' \\\n  -d '{"content": "Build **passed** ✨"}'`}
                </pre>
                <p>{t("serversettings.webhooks.limits", { count: 30 })}</p>
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </div>
  );
}

/** Drifting dots along faint lines behind the header: messages on their way in. */
function Wires() {
  return (
    <svg aria-hidden className="pointer-events-none absolute inset-0 size-full text-primary/20" preserveAspectRatio="none" viewBox="0 0 400 100">
      {[20, 50, 80].map((y, i) => (
        <g key={y}>
          <path d={`M0 ${y} C 120 ${y - 18}, 260 ${y + 18}, 400 ${y}`} fill="none" stroke="currentColor" strokeDasharray="2 6" />
          <motion.circle
            r="2.5"
            className="fill-primary/60"
            initial={{ offsetDistance: "0%" }}
            animate={{ offsetDistance: "100%" }}
            transition={{ duration: 4 + i, repeat: Infinity, ease: "linear", delay: i * 1.3 }}
            style={{ offsetPath: `path("M0 ${y} C 120 ${y - 18}, 260 ${y + 18}, 400 ${y}")` }}
          />
        </g>
      ))}
    </svg>
  );
}

/** One webhook: folded, who it posts as and where; open, everything you can change about it. */
function WebhookCard({
  instanceKey,
  url,
  webhook: w,
  channels,
  creator,
  open,
  onToggle,
  onChange,
  onDelete,
}: {
  instanceKey: string;
  url: string;
  webhook: Webhook;
  channels: Channel[];
  creator: User | undefined;
  open: boolean;
  onToggle: () => void;
  onChange: (w: Webhook) => void;
  onDelete: () => void;
}) {
  const lang = useI18n();
  const { t } = lang;
  const [name, setName] = useState(w.name);
  const [busy, setBusy] = useState<null | "save" | "test" | "reset">(null);
  const [confirm, setConfirm] = useState<null | "reset" | "delete">(null);
  const [shown, setShown] = useState(false);
  const [copied, setCopied] = useState(false);
  const [sent, setSent] = useState(false);
  const shake = useAnimationControls();
  useEffect(() => setName(w.name), [w.name]);
  const channel = channels.find((c) => c.id === w.channelId);
  const messages = Number(w.messages);

  async function save(change: Partial<{ name: string; avatarUrl: string; channelId: string }>) {
    const next = { name: w.name, avatarUrl: w.avatarUrl, channelId: w.channelId, ...change };
    if (next.name === w.name && next.avatarUrl === w.avatarUrl && next.channelId === w.channelId) return;
    setBusy("save");
    try {
      onChange(await run(updateWebhook(instanceKey, w.serverId, w.id, next)));
    } catch (err) {
      toast((err as FuwaError).message);
      setName(w.name);
    }
    setBusy(null);
  }

  function rename() {
    const trimmed = name.trim();
    if (!trimmed) {
      void shake.start({ x: [0, -6, 5, -3, 0], transition: { duration: 0.35 } });
      return setName(w.name);
    }
    void save({ name: trimmed });
  }

  function copyUrl() {
    void navigator.clipboard?.writeText(url).then(
      () => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1400);
      },
      () => toast(t("serversettings.webhooks.copyFailed")),
    );
  }

  async function test() {
    setBusy("test");
    try {
      await run(testWebhook(url, `👋 ${t("serversettings.webhooks.testMessage", { name: `**${w.name}**` })}`));
      setSent(true);
      setTimeout(() => setSent(false), 1600);
    } catch (err) {
      toast((err as FuwaError).message);
    }
    setBusy(null);
  }

  async function reset() {
    setConfirm(null);
    setBusy("reset");
    try {
      onChange(await run(resetWebhookToken(instanceKey, w.serverId, w.id)));
      toast(t("serversettings.webhooks.reset"));
    } catch (err) {
      toast((err as FuwaError).message);
    }
    setBusy(null);
  }

  async function remove() {
    try {
      await run(deleteWebhook(instanceKey, w.serverId, w.id));
      onDelete();
    } catch (err) {
      toast((err as FuwaError).message);
    }
  }

  // Hidden, it's only dots, so nothing of it shows on a shared screen.
  const masked = "•".repeat(28);

  return (
    <motion.li
      layout
      initial={{ opacity: 0, y: 10, scale: 0.97 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, scale: 0.9, filter: "blur(4px)" }}
      transition={SPRING}
      className={cn("overflow-hidden rounded-2xl border bg-background/50 transition-colors", open ? "border-primary/40 shadow-lg shadow-primary/5" : "hover:border-primary/30")}
    >
      <button type="button" onClick={onToggle} aria-expanded={open} className="flex w-full items-center gap-3 p-3 text-left">
        <motion.span whileHover={{ rotate: -8, scale: 1.08 }} transition={{ type: "spring", stiffness: 600, damping: 14 }}>
          <UserAvatar user={asUser(w)} className="size-10" />
        </motion.span>
        <span className="min-w-0 flex-1">
          <span className="flex items-center gap-1.5">
            <span className="truncate font-bold">{w.name}</span>
            <span className="rounded bg-primary/15 px-1 text-[0.6rem] font-extrabold tracking-wide text-primary">APP</span>
          </span>
          <span className="flex items-center gap-1 truncate text-xs text-muted-foreground">
            {channel?.type === ChannelType.ANNOUNCEMENT ? <MegaphoneIcon className="size-3" /> : <HashIcon className="size-3" />}
            <span className="truncate">
              <T
                k={w.lastUsedAt ? "serversettings.webhooks.lineUsed" : "serversettings.webhooks.line"}
                values={{
                  channel: channel?.name ?? t("serversettings.invites.deletedChannel"),
                  count: <Count value={messages} />,
                  when: w.lastUsedAt ? ago(lang, toDate(w.lastUsedAt)) : "",
                }}
                count={messages}
              />
            </span>
          </span>
        </span>
        {busy === "save" && <LoaderCircleIcon className="size-4 animate-spin text-muted-foreground" />}
        <ChevronDownIcon className={cn("size-4 shrink-0 text-muted-foreground transition-transform duration-300", open && "rotate-180")} />
      </button>

      <AnimatePresence initial={false}>
        {open && (
          <motion.div initial={{ height: 0, opacity: 0 }} animate={{ height: "auto", opacity: 1 }} exit={{ height: 0, opacity: 0 }} transition={SPRING} className="overflow-hidden">
            <div className="flex flex-col gap-4 border-t p-4">
              <div className="flex flex-wrap items-start gap-4">
                <PictureField
                  instanceKey={instanceKey}
                  kind="avatar"
                  compact
                  serverId={w.serverId}
                  value={w.avatarUrl}
                  onChange={(avatarUrl) => void save({ avatarUrl })}
                  fallback={<UserAvatar user={asUser({ ...w, avatarUrl: "" })} className="size-full text-2xl" />}
                />
                <div className="flex min-w-0 flex-1 basis-56 flex-col gap-3">
                  <label className="flex flex-col gap-1">
                    <span className="text-xs font-bold text-muted-foreground uppercase">{t("serversettings.overview.name")}</span>
                    <motion.span animate={shake}>
                      <Input
                        value={name}
                        maxLength={80}
                        onChange={(e) => setName(e.target.value)}
                        onBlur={rename}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") e.currentTarget.blur();
                          if (e.key === "Escape") setName(w.name);
                        }}
                        className="h-9 rounded-xl font-bold"
                      />
                    </motion.span>
                  </label>
                  <div className="flex flex-col gap-1">
                    <span className="text-xs font-bold text-muted-foreground uppercase">{t("serversettings.webhooks.postsIn")}</span>
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <button
                          type="button"
                          className="group flex h-9 w-full items-center gap-2 rounded-xl border px-3 text-left text-sm transition hover:border-primary/40 data-[state=open]:border-primary/60"
                        >
                          {channel?.type === ChannelType.ANNOUNCEMENT ? (
                            <MegaphoneIcon className="size-4 text-muted-foreground" />
                          ) : (
                            <HashIcon className="size-4 text-muted-foreground" />
                          )}
                          <span className="flex-1 truncate font-bold">{channel?.name ?? t("serversettings.shared.pickChannel")}</span>
                          <ChevronDownIcon className="size-4 text-muted-foreground transition-transform duration-300 group-data-[state=open]:rotate-180" />
                        </button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="start" className="max-h-72 w-60 overflow-y-auto">
                        {channels.map((c) => (
                          <DropdownMenuItem key={c.id} onSelect={() => void save({ channelId: c.id })}>
                            {c.type === ChannelType.ANNOUNCEMENT ? <MegaphoneIcon /> : <HashIcon />} {c.name}
                            {c.id === w.channelId && <CheckIcon className="ml-auto" />}
                          </DropdownMenuItem>
                        ))}
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>
                </div>
              </div>

              <div className="flex flex-col gap-1">
                <span className="text-xs font-bold text-muted-foreground uppercase">{t("serversettings.webhooks.address")}</span>
                <div className="flex items-center gap-1 rounded-xl border bg-muted/40 p-1 pl-3">
                  <AnimatePresence mode="wait" initial={false}>
                    <motion.code
                      key={shown ? "shown" : "hidden"}
                      initial={{ opacity: 0, filter: "blur(4px)" }}
                      animate={{ opacity: 1, filter: "blur(0px)" }}
                      exit={{ opacity: 0, filter: "blur(4px)" }}
                      transition={{ duration: 0.18 }}
                      className={cn("min-w-0 flex-1 font-mono text-xs", shown ? "break-all" : "truncate")}
                    >
                      {shown ? url : masked}
                    </motion.code>
                  </AnimatePresence>
                  <Button type="button" variant="ghost" size="icon" className="size-8 shrink-0 rounded-lg" aria-label={shown ? t("serversettings.webhooks.hide") : t("serversettings.webhooks.show")} onClick={() => setShown((s) => !s)}>
                    {shown ? <EyeOffIcon /> : <EyeIcon />}
                  </Button>
                  <Button type="button" size="sm" className="btn h-8 shrink-0 rounded-lg px-3 font-bold" onClick={copyUrl}>
                    <AnimatePresence mode="popLayout" initial={false}>
                      <motion.span
                        key={copied ? "done" : "copy"}
                        initial={{ y: 12, opacity: 0 }}
                        animate={{ y: 0, opacity: 1 }}
                        exit={{ y: -12, opacity: 0 }}
                        transition={SPRING}
                        className="flex items-center gap-1.5"
                      >
                        {copied ? <CheckIcon strokeWidth={3} /> : <CopyIcon />} {copied ? t("serversettings.webhooks.copied") : t("serversettings.webhooks.copy")}
                      </motion.span>
                    </AnimatePresence>
                  </Button>
                </div>
                <p className="text-xs text-muted-foreground">{t("serversettings.webhooks.secret")}</p>
              </div>

              <div className="flex flex-wrap items-center gap-2">
                <Button type="button" variant="outline" size="sm" className="rounded-xl font-bold" disabled={!!busy || !channel} onClick={() => void test()}>
                  <AnimatePresence mode="popLayout" initial={false}>
                    <motion.span
                      key={sent ? "sent" : busy === "test" ? "sending" : "send"}
                      initial={{ x: -10, opacity: 0 }}
                      animate={{ x: 0, opacity: 1 }}
                      exit={{ x: 24, y: -10, opacity: 0, rotate: -12 }}
                      transition={SPRING}
                      className="flex items-center gap-1.5"
                    >
                      {sent ? <CheckIcon className="text-emerald-500" strokeWidth={3} /> : busy === "test" ? <LoaderCircleIcon className="animate-spin" /> : <SendIcon />}
                      {sent ? t("serversettings.webhooks.posted") : t("serversettings.webhooks.test")}
                    </motion.span>
                  </AnimatePresence>
                </Button>
                <AnimatePresence mode="popLayout" initial={false}>
                  {confirm ? (
                    <motion.span key="confirm" initial={{ opacity: 0, x: 8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: 8 }} transition={SPRING} className="flex items-center gap-1">
                      <span className="px-1 text-xs font-bold">{confirm === "reset" ? t("serversettings.webhooks.resetAsk") : t("serversettings.webhooks.deleteAsk")}</span>
                      <Button
                        type="button"
                        size="sm"
                        variant="destructive"
                        className="h-8 rounded-full px-3 text-xs font-bold"
                        onClick={() => void (confirm === "reset" ? reset() : remove())}
                      >
                        {confirm === "reset" ? t("serversettings.webhooks.newAddress") : t("serversettings.shared.delete")}
                      </Button>
                      <Button type="button" size="icon" variant="ghost" aria-label={t("serversettings.shared.neverMind")} className="size-8 rounded-full" onClick={() => setConfirm(null)}>
                        <XIcon />
                      </Button>
                    </motion.span>
                  ) : (
                    <motion.span key="actions" initial={{ opacity: 0, x: -8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -8 }} transition={SPRING} className="flex gap-2">
                      <Button type="button" variant="ghost" size="sm" className="rounded-xl" disabled={!!busy} onClick={() => setConfirm("reset")}>
                        <RefreshCwIcon className={cn(busy === "reset" && "animate-spin")} /> {t("serversettings.webhooks.newAddress")}
                      </Button>
                      <Button type="button" variant="ghost" size="sm" className="rounded-xl text-destructive hover:text-destructive" onClick={() => setConfirm("delete")}>
                        <Trash2Icon /> {t("serversettings.shared.delete")}
                      </Button>
                    </motion.span>
                  )}
                </AnimatePresence>
                <span className="ml-auto flex items-center gap-1.5 text-xs text-muted-foreground">
                  <UserAvatar user={creator} className="size-4" />
                  {creator
                    ? t("serversettings.webhooks.madeBy", { name: displayName(creator), when: ago(lang, toDate(w.createdAt)) })
                    : t("serversettings.webhooks.madeBySomeone", { when: ago(lang, toDate(w.createdAt)) })}
                </span>
              </div>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </motion.li>
  );
}
