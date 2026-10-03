import {
  ArrowRightIcon,
  CheckIcon,
  ClipboardPasteIcon,
  CopyIcon,
  DatabaseIcon,
  EyeIcon,
  FolderIcon,
  HashIcon,
  ImageIcon,
  KeyRoundIcon,
  LinkIcon,
  LoaderCircleIcon,
  MessageSquareIcon,
  PlusIcon,
  ScanEyeIcon,
  SendIcon,
  TimerIcon,
  Trash2Icon,
  UndoIcon,
  UnplugIcon,
  XIcon,
} from "lucide-react";
import { AnimatePresence, LayoutGroup, motion } from "motion/react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { SharedConnectionState, type ChannelBlock, type PreviewShareResponse, type ShareCode, type SharedConnection } from "@/gen/fuwa/v1/channel_pb";
import { ChannelType, Permission, type Channel } from "@/gen/fuwa/v1/types_pb";
import {
  acceptShare,
  blockFromChannel,
  createShareCode,
  deleteShareCode,
  disconnectShared,
  listConnections,
  previewShare,
  reviewShare,
  run,
  updateConnection,
} from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAccess, useAction } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { ServerIcon, UserAvatar } from "@/components/Icons";
import { InlineMarkdown } from "@/components/Markdown";
import { Count, SPRING, SwapText } from "@/components/motion";
import { Private } from "@/components/Private";
import { ServerTag, SharedGlyph } from "@/components/chat/Shared";
import { useShake } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { displayName, formatDay, toDate } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { has } from "@/lib/permissions";
import { codeLeft, findShareCode } from "@/lib/shared";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/*
 * Shared channels from a manager's side: adding another server's channel
 * with a code (preview first), requests waiting, channels shared either way
 * and what the other server's people may do, codes that still work, and
 * people kept out. `ChannelShare` is the same for one channel, in its settings.
 */

/** What the home can let a guest server's people do; seeing the channel comes with being shown it. */
const SHAREABLE = [
  { permission: Permission.SEND_MESSAGES, label: "Send messages", icon: MessageSquareIcon },
  { permission: Permission.EMBED_LINKS, label: "Embed links", icon: LinkIcon },
  { permission: Permission.ATTACH_FILES, label: "Attach files", icon: ImageIcon },
] as const;

const waiting = (c: SharedConnection) => c.state === SharedConnectionState.WAITING;
const NO_CHANNELS: Channel[] = [];

/** The server's shared channels, read when shown and again whenever the server says they changed. */
export function useSharedList(instanceKey: string, serverId: string) {
  const list = useFuwa((s) => s.instances[instanceKey]?.shared[serverId]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    run(listConnections(instanceKey, serverId)).then(
      () => setError(null),
      (err: FuwaError) => setError(err.message),
    );
  }, [instanceKey, serverId]);
  return { list, error };
}

/** Whether sharing is on for the instance. Off, nothing new starts; what's shared already keeps working. */
export const useSharingOn = (instanceKey: string) => useFuwa((s) => !!s.instances[instanceKey]?.node?.sharedChannels);

/** The server settings page. */
export function SharedChannels({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const { list, error } = useSharedList(instanceKey, serverId);
  const on = useSharingOn(instanceKey);
  const channels = useFuwa((s) => s.instances[instanceKey]?.channels[serverId] ?? NO_CHANNELS);
  const access = useAccess(instanceKey, serverId);
  const canKick = has(access, Permission.KICK_MEMBERS);
  const now = useNow(60_000);
  const nameOf = (id: string) => channels.find((c) => c.id === id)?.name;

  const requests = list?.connections.filter(waiting) ?? [];
  const active = list?.connections.filter((c) => !waiting(c)) ?? [];
  const codes = list?.codes.filter((c) => toDate(c.expiresAt).getTime() > now) ?? [];

  return (
    <div className="flex flex-col gap-8">
      {on ? (
        <AddChannel instanceKey={instanceKey} serverId={serverId} channels={channels} />
      ) : (
        <p className="rounded-2xl border border-dashed p-4 text-sm text-muted-foreground">
          Sharing channels is turned off on this instance, so nothing new can be shared. Channels already shared keep working until
          either server ends them.
        </p>
      )}
      {error ? (
        <p className="text-sm text-muted-foreground first-letter:uppercase">{error}</p>
      ) : !list ? (
        <div className="flex flex-col gap-2">
          {[0, 1, 2].map((n) => (
            <div key={n} className="shimmer h-16 rounded-2xl" />
          ))}
        </div>
      ) : (
        <LayoutGroup id={`shared-${serverId}`}>
          <Section title="Waiting" count={requests.length} hidden={!requests.length}>
            {requests.map((c, n) => (
              <ConnectionRow key={c.id} index={n} instanceKey={instanceKey} serverId={serverId} connection={c} here={nameOf(c.channelId)} />
            ))}
          </Section>
          <Section
            title="Shared channels"
            count={active.length}
            empty={
              <Empty icon={<SharedGlyph className="size-6" />} title="Nothing shared yet">
                Share one of your channels from its settings (Share tab), or add another server's channel with the code its admins give you.
              </Empty>
            }
          >
            {active.map((c, n) => (
              <ConnectionRow key={c.id} index={n} instanceKey={instanceKey} serverId={serverId} connection={c} here={nameOf(c.channelId)} />
            ))}
          </Section>
          <Section title="Share codes" count={codes.length} hidden={!codes.length}>
            {codes.map((code, n) => (
              <CodeRow key={code.code} index={n} instanceKey={instanceKey} serverId={serverId} code={code} now={now} />
            ))}
          </Section>
          <Section title="People kept out" count={list.blocks.length} hidden={!list.blocks.length}>
            {list.blocks.map((b, n) => (
              <BlockRow key={`${b.channelId}/${b.user?.id}`} index={n} instanceKey={instanceKey} serverId={serverId} block={b} channelName={nameOf(b.channelId)} canKick={canKick} />
            ))}
          </Section>
        </LayoutGroup>
      )}
    </div>
  );
}

function Section({ title, count, hidden = false, empty, children }: { title: string; count: number; hidden?: boolean; empty?: ReactNode; children: ReactNode }) {
  return (
    <AnimatePresence initial={false}>
      {!hidden && (
        <motion.section
          key={title}
          layout="position"
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -4, transition: { duration: 0.15 } }}
          transition={SPRING}
          className="flex flex-col gap-2"
        >
          <h3 className="flex items-center gap-1.5 text-xs font-bold tracking-wide text-muted-foreground uppercase">
            {title}
            {count > 0 && (
              <span className="rounded-full bg-muted px-1.5 text-[0.65rem] tabular-nums">
                <Count value={count} />
              </span>
            )}
          </h3>
          {count === 0 && empty ? empty : (
            <ul className="flex flex-col gap-2">
              <AnimatePresence initial={false} mode="popLayout">
                {children}
              </AnimatePresence>
            </ul>
          )}
        </motion.section>
      )}
    </AnimatePresence>
  );
}

function Empty({ icon, title, children }: { icon: ReactNode; title: string; children: ReactNode }) {
  return (
    <motion.div
      initial={{ opacity: 0, scale: 0.97 }}
      animate={{ opacity: 1, scale: 1 }}
      transition={SPRING}
      className="flex flex-col items-center gap-2 rounded-3xl border border-dashed p-8 text-center"
    >
      <span className="float grid size-12 place-items-center rounded-full bg-primary/15 text-primary">{icon}</span>
      <p className="font-bold">{title}</p>
      <p className="max-w-sm text-sm text-muted-foreground">{children}</p>
    </motion.div>
  );
}

// ───────────────────────── Adding another server's channel ─────────────────────────

/** Channel names are lowercase with dashes, like the server makes them. */
const slug = (name: string) => name.toLowerCase().replace(/\s+/g, "-").replace(/[^\p{L}\p{N}_-]/gu, "").slice(0, 100);

/** Paste a code, see where it leads, pick a name and a place, and ask. */
function AddChannel({ instanceKey, serverId, channels }: { instanceKey: string; serverId: string; channels: Channel[] }) {
  const [text, setText] = useState("");
  const code = findShareCode(text);
  const [preview, setPreview] = useState<PreviewShareResponse | null>(null);
  const [name, setName] = useState("");
  const [parentId, setParentId] = useState("");
  const look = useAction(previewShare);
  const ask = useAction(acceptShare);
  const [shake, nudge] = useShake();
  const categories = channels.filter((c) => c.type === ChannelType.CATEGORY);

  async function lookUp() {
    if (!code) {
      look.setError("that doesn't look like a share code");
      nudge();
      return;
    }
    const found = await look.go(instanceKey, serverId, code);
    if (!found) return nudge();
    setPreview(found);
    setName(found.channelName);
  }

  async function connect() {
    if (!preview) return;
    const chosen = slug(name);
    const done = await ask.go(instanceKey, serverId, code, chosen === preview.channelName ? "" : chosen, parentId);
    if (!done) return nudge();
    toast(`Asked ${preview.homeServer?.name ?? "the other server"}. #${chosen || preview.channelName} shows up here once they approve.`);
    setText("");
    setPreview(null);
    setParentId("");
  }

  const parentName = categories.find((c) => c.id === parentId)?.name ?? "No category";
  return (
    <motion.section layout="position" transition={SPRING} className="flex flex-col gap-3 rounded-3xl border bg-background/40 p-4 sm:p-5">
      <div className="flex items-start gap-3">
        <span className="grid size-10 shrink-0 place-items-center rounded-2xl bg-primary/15 text-primary">
          <SharedGlyph className="size-5" />
        </span>
        <div className="min-w-0">
          <h3 className="font-extrabold">Add a channel from another server</h3>
          <p className="text-sm text-muted-foreground">Paste the share code its admins gave you. You'll see where it leads before anything changes.</p>
        </div>
      </div>
      <motion.form
        animate={shake}
        onSubmit={(e) => {
          e.preventDefault();
          void lookUp();
        }}
        className="flex flex-col gap-2 sm:flex-row"
      >
        <div className="relative min-w-0 flex-1">
          <KeyRoundIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            aria-label="Share code"
            placeholder="Paste a share code"
            value={text}
            spellCheck={false}
            autoComplete="off"
            onChange={(e) => {
              // Pasted with words around it: keep just the code.
              setText(findShareCode(e.target.value) || e.target.value);
              look.setError(null);
              ask.setError(null);
              if (preview && findShareCode(e.target.value) !== code) setPreview(null);
            }}
            className="h-11 rounded-xl pl-9 font-mono"
          />
        </div>
        <Button type="submit" disabled={look.pending || !text.trim()} className="btn h-11 rounded-xl font-bold">
          {look.pending ? <LoaderCircleIcon className="animate-spin" /> : <ScanEyeIcon />} Preview
        </Button>
      </motion.form>
      <AnimatePresence initial={false}>
        {look.error && (
          <motion.p
            key="error"
            initial={{ opacity: 0, y: -4 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -4 }}
            transition={SPRING}
            className="text-sm text-destructive first-letter:uppercase"
          >
            {look.error}
          </motion.p>
        )}
      </AnimatePresence>
      <AnimatePresence mode="popLayout" initial={false}>
        {preview && (
          <motion.div
            key={code}
            initial={{ opacity: 0, y: 24, scale: 0.96 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 12, scale: 0.97, transition: { duration: 0.18 } }}
            transition={{ type: "spring", stiffness: 380, damping: 28 }}
            className="flex flex-col gap-4"
          >
            <PreviewCard preview={preview} />
            <div className="grid gap-3 sm:grid-cols-2">
              <label className="flex flex-col gap-1.5">
                <span className="text-sm font-extrabold">Name here</span>
                <span className="relative">
                  <HashIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
                  <Input value={slug(name)} maxLength={100} onChange={(e) => setName(e.target.value)} className="h-11 rounded-xl pl-9" />
                </span>
              </label>
              <div className="flex flex-col gap-1.5">
                <span className="text-sm font-extrabold">Category</span>
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <button
                      type="button"
                      className="flex h-11 items-center gap-2 rounded-xl border px-3 text-left text-sm transition hover:border-primary/40 data-[state=open]:border-primary/60"
                    >
                      <FolderIcon className="size-4 text-muted-foreground" />
                      <span className="flex-1 truncate font-bold">{parentName}</span>
                    </button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="start" className="w-64">
                    <DropdownMenuRadioGroup value={parentId} onValueChange={setParentId}>
                      <DropdownMenuRadioItem value="">No category</DropdownMenuRadioItem>
                      {categories.map((c) => (
                        <DropdownMenuRadioItem key={c.id} value={c.id}>
                          {c.name}
                        </DropdownMenuRadioItem>
                      ))}
                    </DropdownMenuRadioGroup>
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
            </div>
            {ask.error && <p className="text-sm text-destructive first-letter:uppercase">{ask.error}</p>}
            <div className="flex flex-wrap items-center justify-end gap-2">
              <p className="mr-auto text-xs text-muted-foreground">Their admins approve it before it shows up here.</p>
              <Button type="button" variant="ghost" onClick={() => setPreview(null)} className="rounded-xl">
                Not now
              </Button>
              <Button type="button" disabled={ask.pending || !slug(name)} onClick={() => void connect()} className="btn group rounded-xl font-bold">
                {ask.pending ? <LoaderCircleIcon className="animate-spin" /> : <SendIcon className="transition-transform group-hover:translate-x-0.5 group-hover:-translate-y-0.5" />}
                Ask to connect
              </Button>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </motion.section>
  );
}

const stagger = (n: number) => ({ initial: { opacity: 0, y: 8 }, animate: { opacity: 1, y: 0, transition: { ...SPRING, delay: 0.12 + n * 0.05 } } });

/** Where a code leads: whose channel it is, where what's said is kept, and what your people may do there. */
function PreviewCard({ preview }: { preview: PreviewShareResponse }) {
  const home = preview.homeServer;
  const now = useNow(60_000);
  const left = toDate(preview.expiresAt).getTime() - now;
  return (
    <div className="overflow-hidden rounded-2xl border border-primary/30 bg-card shadow-sm">
      <div className="flex items-center gap-3 bg-primary/8 p-4">
        <motion.span initial={{ scale: 0.4, rotate: -20 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 420, damping: 16, delay: 0.05 }}>
          {home && <ServerIcon server={home} className="size-12 rounded-2xl" />}
        </motion.span>
        <div className="min-w-0 flex-1">
          <p className="text-xs font-bold text-muted-foreground">From {home?.name ?? "another server"}</p>
          <p className="flex items-center gap-1 truncate text-lg font-extrabold">
            <HashIcon className="size-4 shrink-0 text-muted-foreground" />
            <span className="truncate">{preview.channelName}</span>
          </p>
        </div>
        {left > 0 && (
          <span className="hidden shrink-0 items-center gap-1 rounded-full bg-muted px-2 py-0.5 text-xs text-muted-foreground sm:flex">
            <TimerIcon className="size-3" /> Code works for {codeLeft(left)}
          </span>
        )}
      </div>
      <div className="flex flex-col gap-3 p-4 text-sm">
        {preview.channelTopic && (
          <motion.div {...stagger(0)}>
            <InlineMarkdown className="text-muted-foreground">{preview.channelTopic}</InlineMarkdown>
          </motion.div>
        )}
        <motion.p {...stagger(1)} className="flex items-start gap-2">
          <DatabaseIcon className="mt-0.5 size-4 shrink-0 text-primary" />
          <span>
            Messages are stored only on <b>{home?.name ?? "the other server"}</b>
            {preview.region && <> ({preview.region})</>}. Your people's messages there are kept by them too, even if you disconnect later.
          </span>
        </motion.p>
        {preview.checkedBy.length > 0 && (
          <motion.p {...stagger(2)} className="flex items-start gap-2">
            <EyeIcon className="mt-0.5 size-4 shrink-0 text-primary" />
            <span>
              Their AutoMod also sends what's written there to <b>{preview.checkedBy.join(", ")}</b>.
            </span>
          </motion.p>
        )}
        {preview.guestCount > 0 && (
          <motion.p {...stagger(3)} className="flex items-start gap-2 text-muted-foreground">
            <SharedGlyph className="mt-0.5 size-4 shrink-0" />
            Already shown in {preview.guestCount} other {preview.guestCount === 1 ? "server" : "servers"}.
          </motion.p>
        )}
        <motion.div {...stagger(4)} className="flex flex-col gap-1.5">
          <p className="font-bold">What your people can do there</p>
          <ul className="flex flex-wrap gap-1.5">
            <Capability on label="Read" icon={EyeIcon} index={0} />
            {SHAREABLE.map((s, n) => (
              <Capability key={s.permission} on={preview.allowed.includes(s.permission)} label={s.label} icon={s.icon} index={n + 1} />
            ))}
          </ul>
          <p className="text-xs text-muted-foreground">You decide which of your people see it with roles here. Pings to @everyone or roles never reach the other server.</p>
        </motion.div>
      </div>
    </div>
  );
}

function Capability({ on, label, icon: Icon, index }: { on: boolean; label: string; icon: typeof EyeIcon; index: number }) {
  return (
    <motion.li
      initial={{ opacity: 0, scale: 0.8 }}
      animate={{ opacity: 1, scale: 1, transition: { type: "spring", stiffness: 500, damping: 22, delay: 0.3 + index * 0.05 } }}
      className={cn(
        "flex items-center gap-1 rounded-full border px-2.5 py-1 text-xs font-bold",
        on ? "border-primary/30 bg-primary/10 text-primary" : "text-muted-foreground line-through decoration-muted-foreground/50",
      )}
    >
      {on ? <Icon className="size-3.5" /> : <XIcon className="size-3.5" />}
      {label}
    </motion.li>
  );
}

// ───────────────────────── Rows ─────────────────────────

const rowMotion = (index: number) => ({
  initial: { opacity: 0, y: 10 },
  animate: { opacity: 1, y: 0, transition: { ...SPRING, delay: Math.min(index, 10) * 0.03 } },
  exit: { opacity: 0, scale: 0.96, x: 24, transition: { duration: 0.2 } },
});

/** Waiting or connected: a chip that swaps when the other side answers. */
function StateChip({ connection }: { connection: SharedConnection }) {
  const isWaiting = waiting(connection);
  return (
    <motion.span
      layout
      transition={SPRING}
      className={cn(
        "inline-flex shrink-0 items-center gap-1.5 rounded-full px-2 py-0.5 text-xs font-bold transition-colors duration-300",
        isWaiting ? "bg-muted text-muted-foreground" : "bg-primary/12 text-primary",
      )}
    >
      <AnimatePresence mode="popLayout" initial={false}>
        {isWaiting ? (
          <motion.span key="dot" initial={{ scale: 0 }} animate={{ scale: 1 }} exit={{ scale: 0 }} transition={SPRING} className="relative grid size-2 place-items-center">
            <span className="absolute inset-0 animate-ping rounded-full bg-primary/50" />
            <span className="relative size-2 rounded-full bg-primary" />
          </motion.span>
        ) : (
          <motion.span key="check" initial={{ scale: 0, rotate: -60 }} animate={{ scale: 1, rotate: 0 }} exit={{ scale: 0 }} transition={{ type: "spring", stiffness: 600, damping: 16 }}>
            <CheckIcon className="size-3" strokeWidth={3} />
          </motion.span>
        )}
      </AnimatePresence>
      <SwapText>{isWaiting ? "Waiting" : "Connected"}</SwapText>
    </motion.span>
  );
}

type Ask = "disconnect" | "turn-down" | "cancel";

/** What a connection is, in a line, from this server's side. */
function ConnectionLine({ connection: c, homeName, here }: { connection: SharedConnection; homeName: string; here: string | undefined }) {
  if (c.home) return waiting(c) ? <>wants to show <b>#{homeName}</b> in their server</> : <>sees <b>#{homeName}</b></>;
  if (waiting(c)) return <>Waiting for them to approve <b>#{homeName}</b></>;
  return (
    <>
      <b>#{homeName}</b>, here as <b>#{here ?? homeName}</b>
    </>
  );
}

/** What the confirm dialog says before ending a connection or a request. */
function askCopy(ask: Ask, c: SharedConnection, otherName: string, homeName: string, here: string | undefined) {
  if (ask === "turn-down")
    return { title: `Turn down ${otherName}?`, body: `#${homeName} won't show up in their server. They can ask again with a new code.`, action: "Turn down", done: `Turned down ${otherName}` };
  if (ask === "cancel")
    return { title: "Cancel this request?", body: `${otherName} won't see the request anymore. You can ask again with a new code.`, action: "Cancel request", done: "Request canceled" };
  return c.home
    ? {
        title: `Stop sharing #${homeName} with ${otherName}?`,
        body: `It goes away from ${otherName}. Everything said stays here, their people's messages included.`,
        action: "Disconnect",
        done: `Disconnected from ${otherName}`,
      }
    : {
        title: `Remove #${here ?? homeName} from this server?`,
        body: `The channel goes away here. Everything said stays on ${otherName}, your people's messages included.`,
        action: "Disconnect",
        done: `Disconnected from ${otherName}`,
      };
}

/** One connection, from this server's side, with what you can do about it. */
export function ConnectionRow({
  instanceKey,
  serverId,
  connection: c,
  here,
  index,
}: {
  instanceKey: string;
  serverId: string;
  connection: SharedConnection;
  /** The channel's name in this server, once there is one. */
  here: string | undefined;
  index: number;
}) {
  const otherName = c.server?.name ?? "another server";
  const [confirm, setConfirm] = useState<Ask | null>(null);
  const homeName = c.homeChannelName || here || "a channel";
  const ask = confirm ? askCopy(confirm, c, otherName, homeName, here) : null;

  return (
    <motion.li
      layoutId={`connection-${c.id}`}
      {...rowMotion(index)}
      className="flex flex-col gap-3 rounded-2xl border bg-background/40 p-3 transition-colors hover:border-primary/30"
    >
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <span className="relative shrink-0">
          {c.server && <ServerIcon server={c.server} className="size-10 rounded-xl" />}
          <span className="absolute -right-1 -bottom-1 grid size-5 place-items-center rounded-full bg-background text-primary ring-2 ring-background">
            <SharedGlyph className="size-3" />
          </span>
        </span>
        <div className="min-w-0 flex-1 basis-48">
          <p className="truncate font-bold">{c.home ? otherName : `From ${otherName}`}</p>
          <p className="truncate text-sm text-muted-foreground">
            <ConnectionLine connection={c} homeName={homeName} here={here} />
          </p>
        </div>
        <StateChip connection={c} />
        <ConnectionActions instanceKey={instanceKey} serverId={serverId} connection={c} onAsk={setConfirm} approved={`#${homeName} is now shared with ${otherName}`} />
      </div>
      {c.checkedBy.length > 0 && (
        <motion.p
          initial={{ opacity: 0, y: 4 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ type: "spring", stiffness: 420, damping: 32 }}
          className="flex items-start gap-2 text-sm text-muted-foreground"
        >
          <EyeIcon className="mt-0.5 size-4 shrink-0 text-primary" />
          <span>
            {c.home ? "Your" : "Their"} AutoMod sends what's written there to <b className="text-foreground">{c.checkedBy.join(", ")}</b>
            {c.home ? ", their people's messages included." : ", your people's messages included."}
          </span>
        </motion.p>
      )}
      {!waiting(c) && <Allowed instanceKey={instanceKey} serverId={serverId} connection={c} />}
      <ConfirmDialog
        open={confirm !== null}
        onOpenChange={(open) => !open && setConfirm(null)}
        title={ask?.title ?? ""}
        body={ask?.body ?? ""}
        action={ask?.action ?? ""}
        onConfirm={async () => {
          if (confirm === "turn-down") await run(reviewShare(instanceKey, serverId, c.id, false));
          else await run(disconnectShared(instanceKey, serverId, c.id));
          if (ask) toast(ask.done);
        }}
      />
    </motion.li>
  );
}

/** Approve or turn down a request (at the home), or end a connection or request. */
function ConnectionActions({
  instanceKey,
  serverId,
  connection: c,
  onAsk,
  approved,
}: {
  instanceKey: string;
  serverId: string;
  connection: SharedConnection;
  onAsk: (ask: Ask) => void;
  approved: string;
}) {
  const [approving, setApproving] = useState(false);
  if (c.home && waiting(c))
    return (
      <span className="flex shrink-0 items-center gap-1.5">
        <Button type="button" size="sm" variant="ghost" onClick={() => onAsk("turn-down")} className="rounded-xl">
          Turn down
        </Button>
        <Button
          type="button"
          size="sm"
          disabled={approving}
          title="Their people will be able to read the channel, past messages included"
          onClick={async () => {
            setApproving(true);
            try {
              await run(reviewShare(instanceKey, serverId, c.id, true));
              toast(approved);
            } catch (err) {
              toast((err as FuwaError).message);
            } finally {
              setApproving(false);
            }
          }}
          className="btn rounded-xl font-bold"
        >
          {approving ? <LoaderCircleIcon className="animate-spin" /> : <CheckIcon />} Approve
        </Button>
      </span>
    );
  const pending = waiting(c);
  return (
    <Button
      type="button"
      size="sm"
      variant="ghost"
      onClick={() => onAsk(pending ? "cancel" : "disconnect")}
      className="group shrink-0 rounded-xl text-destructive hover:bg-destructive/10 hover:text-destructive"
    >
      {pending ? <XIcon className="transition-transform group-hover:rotate-90" /> : <UnplugIcon className="transition-transform group-hover:-rotate-12" />}
      {pending ? "Cancel" : "Disconnect"}
    </Button>
  );
}

/**
 * What the guest server's people may do in the channel. The home switches
 * each on or off; the guest sees what they were given.
 */
function Allowed({ instanceKey, serverId, connection: c }: { instanceKey: string; serverId: string; connection: SharedConnection }) {
  const [saving, setSaving] = useState<Permission | null>(null);
  if (!c.home)
    return (
      <ul className="flex flex-wrap gap-1.5 pl-[3.25rem]">
        {SHAREABLE.map((s, n) => (
          <Capability key={s.permission} on={c.allowed.includes(s.permission)} label={s.label} icon={s.icon} index={n} />
        ))}
      </ul>
    );
  async function toggle(permission: Permission, on: boolean) {
    const next = on ? [...c.allowed, permission] : c.allowed.filter((p) => p !== permission);
    setSaving(permission);
    try {
      await run(updateConnection(instanceKey, serverId, c.id, next));
    } catch (err) {
      toast((err as FuwaError).message);
    } finally {
      setSaving(null);
    }
  }
  return (
    <div className="grid gap-1.5 rounded-xl bg-muted/40 p-2 sm:grid-cols-3">
      {SHAREABLE.map((s) => {
        const on = c.allowed.includes(s.permission);
        return (
          <label key={s.permission} className="flex cursor-pointer items-center gap-2 rounded-lg px-2 py-1.5 text-sm transition-colors hover:bg-background/60">
            <motion.span animate={{ scale: on ? 1 : 0.9, opacity: on ? 1 : 0.5 }} transition={SPRING} className={cn("grid size-6 place-items-center rounded-md", on ? "bg-primary/15 text-primary" : "bg-muted text-muted-foreground")}>
              <s.icon className="size-3.5" />
            </motion.span>
            <span className="min-w-0 flex-1 truncate font-bold">{s.label}</span>
            {saving === s.permission ? (
              <LoaderCircleIcon className="size-4 animate-spin text-muted-foreground" />
            ) : (
              <Switch checked={on} disabled={saving !== null} onCheckedChange={(v) => void toggle(s.permission, v)} aria-label={`${s.label} (their people)`} />
            )}
          </label>
        );
      })}
    </div>
  );
}

function CodeRow({ instanceKey, serverId, code, now, index }: { instanceKey: string; serverId: string; code: ShareCode; now: number; index: number }) {
  const left = toDate(code.expiresAt).getTime() - now;
  const [deleting, setDeleting] = useState(false);
  return (
    <motion.li {...rowMotion(index)} layout className="flex flex-wrap items-center gap-x-4 gap-y-2 rounded-2xl border bg-background/40 p-3">
      <span className="grid size-9 shrink-0 place-items-center rounded-xl bg-primary/12 text-primary">
        <KeyRoundIcon className="size-4" />
      </span>
      <span className="min-w-0 flex-1 basis-48">
        <span className="block truncate text-sm font-bold">#{code.channelName}</span>
        <span className="block truncate font-mono text-xs text-muted-foreground">
          <Private text={code.code} kind="secret" />
        </span>
      </span>
      <span className={cn("flex items-center gap-1 text-sm tabular-nums", left < 86_400_000 ? "text-destructive" : "text-muted-foreground")} title={`Works until ${formatDay(toDate(code.expiresAt))}`}>
        <TimerIcon className="size-3.5" /> {codeLeft(left)}
      </span>
      <span className="ml-auto flex items-center gap-1">
        <CopyButton text={code.code} label="Copy code" />
        <button
          type="button"
          aria-label="Delete code"
          title="Delete code"
          disabled={deleting}
          onClick={async () => {
            setDeleting(true);
            await run(deleteShareCode(instanceKey, serverId, code.code)).catch((err: FuwaError) => {
              toast(err.message);
              setDeleting(false);
            });
          }}
          className="grid size-8 place-items-center rounded-lg text-muted-foreground transition hover:rotate-12 hover:bg-destructive/10 hover:text-destructive active:scale-90 disabled:opacity-50"
        >
          {deleting ? <LoaderCircleIcon className="size-4 animate-spin" /> : <Trash2Icon className="size-4" />}
        </button>
      </span>
    </motion.li>
  );
}

function BlockRow({
  instanceKey,
  serverId,
  block,
  channelName,
  canKick,
  index,
}: {
  instanceKey: string;
  serverId: string;
  block: ChannelBlock;
  channelName: string | undefined;
  canKick: boolean;
  index: number;
}) {
  const [lifting, setLifting] = useState(false);
  const name = displayName(block.user);
  return (
    <motion.li {...rowMotion(index)} layout className="group flex flex-wrap items-center gap-3 rounded-2xl border bg-background/40 p-3">
      <UserAvatar user={block.user} className="size-10 grayscale transition duration-300 group-hover:grayscale-0" />
      <div className="min-w-0 flex-1 basis-40">
        <p className="flex min-w-0 items-center gap-1.5 font-bold">
          <span className="truncate">{name}</span>
          {block.server && <ServerTag server={block.server} />}
        </p>
        <p className="truncate text-xs text-muted-foreground">
          Kept out of #{channelName ?? "a channel"} · {formatDay(toDate(block.createdAt))}
        </p>
      </div>
      {canKick && (
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={lifting}
          onClick={async () => {
            setLifting(true);
            try {
              await run(blockFromChannel(instanceKey, serverId, block.channelId, block.user?.id ?? "", false));
              toast(`${name} can see #${channelName ?? "the channel"} again`);
            } catch (err) {
              toast((err as FuwaError).message);
              setLifting(false);
            }
          }}
          className="group/back shrink-0 rounded-xl"
        >
          {lifting ? <LoaderCircleIcon className="animate-spin" /> : <UndoIcon className="transition-transform duration-300 group-hover/back:-rotate-45" />}
          Let back in
        </Button>
      )}
    </motion.li>
  );
}

function CopyButton({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={async () => {
        try {
          await navigator.clipboard?.writeText(text);
          setCopied(true);
          setTimeout(() => setCopied(false), 1400);
        } catch {
          toast("Couldn't copy it");
        }
      }}
      className={cn("grid size-8 place-items-center rounded-lg text-muted-foreground transition hover:bg-muted hover:text-foreground active:scale-90", copied && "text-primary hover:text-primary")}
    >
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.span key={String(copied)} initial={{ scale: 0, rotate: -45 }} animate={{ scale: 1, rotate: 0 }} exit={{ scale: 0 }} transition={SPRING}>
          {copied ? <CheckIcon className="size-4" strokeWidth={3} /> : <CopyIcon className="size-4" />}
        </motion.span>
      </AnimatePresence>
    </button>
  );
}

/** Asks before something that can't be taken back in one click. Shows the server's answer if it says no. */
function ConfirmDialog({
  open,
  onOpenChange,
  title,
  body,
  action,
  onConfirm,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  body: string;
  action: string;
  onConfirm: () => Promise<void>;
}) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setError(null);
        onOpenChange(next);
      }}
    >
      <DialogContent>
        <DialogHeader title={title} description={body} />
        {error && <p className="mb-3 text-sm text-destructive first-letter:uppercase">{error}</p>}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)} className="rounded-xl">
            Keep it
          </Button>
          <Button
            type="button"
            variant="destructive"
            disabled={pending}
            onClick={async () => {
              setPending(true);
              setError(null);
              try {
                await onConfirm();
                onOpenChange(false);
              } catch (err) {
                setError((err as FuwaError).message);
              } finally {
                setPending(false);
              }
            }}
            className="rounded-xl font-bold"
          >
            {pending && <LoaderCircleIcon className="animate-spin" />} {action}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

// ───────────────────────── One channel's Share tab ─────────────────────────

/**
 * Sharing one channel, in its settings: a code for another server's admins,
 * the codes still out, and the server it's shared with. A channel shown from
 * another server says where it comes from instead.
 */
export function ChannelShare({ instanceKey, serverId, channel }: { instanceKey: string; serverId: string; channel: Channel }) {
  const { list, error } = useSharedList(instanceKey, serverId);
  const guests = useMemo(() => list?.connections.filter((c) => c.channelId === channel.id) ?? [], [list, channel.id]);
  const shown = channel.shared && !channel.shared.home;

  if (error) return <p className="text-sm text-muted-foreground first-letter:uppercase">{error}</p>;
  if (shown) return <ShownChannelShare instanceKey={instanceKey} serverId={serverId} channel={channel} guests={guests} />;
  return <HomeChannelShare instanceKey={instanceKey} serverId={serverId} channel={channel} guests={guests} loaded={!!list} codes={list?.codes ?? []} />;
}

type ShareProps = { instanceKey: string; serverId: string; channel: Channel; guests: SharedConnection[] };

/** A channel shown here from another server: where it comes from, and the way to let it go. */
function ShownChannelShare({ instanceKey, serverId, channel, guests }: ShareProps) {
  const home = channel.shared?.homeServer;
  return (
      <div className="flex flex-col gap-4">
        <p className="text-sm text-muted-foreground">
          This channel comes from <b className="text-foreground">{home?.name ?? "another server"}</b>, where its messages are kept. You decide which of your
          people see it with this channel's permissions; they decide what your people may do there.
        </p>
        <ul className="flex flex-col gap-2">
          <AnimatePresence initial={false} mode="popLayout">
            {guests.map((c, n) => (
              <ConnectionRow key={c.id} index={n} instanceKey={instanceKey} serverId={serverId} connection={c} here={channel.name} />
            ))}
          </AnimatePresence>
        </ul>
      </div>
  );
}

/** One of this server's channels: a code to share it, codes still out, and who it's shared with. */
function HomeChannelShare({ instanceKey, serverId, channel, guests, loaded, codes: all }: ShareProps & { loaded: boolean; codes: ShareCode[] }) {
  const on = useSharingOn(instanceKey);
  const make = useAction(createShareCode);
  const [fresh, setFresh] = useState<ShareCode | null>(null);
  const now = useNow(60_000);
  const codes = all.filter((c) => c.channelId === channel.id && toDate(c.expiresAt).getTime() > now && c.code !== fresh?.code);
  // One other server per channel, for now.
  const taken = guests.length > 0;
  return (
    <div className="flex flex-col gap-5">
      <div className="flex flex-col gap-3 rounded-2xl border bg-background/40 p-4">
        <div className="flex items-start gap-3">
          <span className="grid size-10 shrink-0 place-items-center rounded-2xl bg-primary/15 text-primary">
            <SharedGlyph className="size-5" />
          </span>
          <div className="min-w-0 text-sm">
            <p className="font-extrabold">Share #{channel.name} with another server</p>
            <p className="text-muted-foreground">
              Give a share code to the other server's admins. They'll see this server's name, #{channel.name} and its topic, and what their people
              may do, then ask to connect. Nothing is shared until you approve. Once you do, their people can read
              everything said here, past messages included. Messages stay here.
            </p>
          </div>
        </div>
        <AnimatePresence mode="popLayout" initial={false}>
          {fresh ? (
            <motion.div
              key="code"
              initial={{ opacity: 0, y: 12, scale: 0.96 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={{ opacity: 0, scale: 0.96 }}
              transition={{ type: "spring", stiffness: 420, damping: 26 }}
              className="flex flex-col gap-2 rounded-xl border border-primary/30 bg-primary/5 p-3"
            >
              <div className="flex items-center gap-2">
                <code className="min-w-0 flex-1 font-mono text-sm font-bold break-all">
                  <Private text={fresh.code} kind="secret" className="text-clip whitespace-normal" />
                </code>
                <CopyButton text={fresh.code} label="Copy code" />
              </div>
              <p className="flex items-center gap-1 text-xs text-muted-foreground">
                <TimerIcon className="size-3" /> Works for {codeLeft(toDate(fresh.expiresAt).getTime() - now)}, for one server.
              </p>
            </motion.div>
          ) : on && !taken ? (
            <motion.div key="make" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}>
              <Button
                type="button"
                disabled={make.pending}
                onClick={async () => {
                  const code = await make.go(instanceKey, serverId, channel.id);
                  if (code) setFresh(code);
                }}
                className="btn rounded-xl font-bold"
              >
                {make.pending ? <LoaderCircleIcon className="animate-spin" /> : <PlusIcon />} Create share code
              </Button>
            </motion.div>
          ) : (
            <motion.p key="why" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} className="text-xs text-muted-foreground">
              {!on ? "Sharing is turned off on this instance." : "A channel can be shared with one other server for now."}
            </motion.p>
          )}
        </AnimatePresence>
        {make.error && <p className="text-sm text-destructive first-letter:uppercase">{make.error}</p>}
      </div>
      {codes.length > 0 && (
        <div className="flex flex-col gap-2">
          <h4 className="text-xs font-bold tracking-wide text-muted-foreground uppercase">Codes still out</h4>
          <ul className="flex flex-col gap-2">
            <AnimatePresence initial={false} mode="popLayout">
              {codes.map((code, n) => (
                <CodeRow key={code.code} index={n} instanceKey={instanceKey} serverId={serverId} code={code} now={now} />
              ))}
            </AnimatePresence>
          </ul>
        </div>
      )}
      <div className="flex flex-col gap-2">
        <h4 className="text-xs font-bold tracking-wide text-muted-foreground uppercase">Shared with</h4>
        {!loaded ? (
          <div className="shimmer h-16 rounded-2xl" />
        ) : guests.length === 0 ? (
          <p className="flex items-center gap-2 text-sm text-muted-foreground">
            <ClipboardPasteIcon className="size-4" /> No other server yet. <ArrowRightIcon className="size-3.5" /> Send a code to start.
          </p>
        ) : (
          <ul className="flex flex-col gap-2">
            <AnimatePresence initial={false} mode="popLayout">
              {guests.map((c, n) => (
                <ConnectionRow key={c.id} index={n} instanceKey={instanceKey} serverId={serverId} connection={c} here={channel.name} />
              ))}
            </AnimatePresence>
          </ul>
        )}
      </div>
    </div>
  );
}
