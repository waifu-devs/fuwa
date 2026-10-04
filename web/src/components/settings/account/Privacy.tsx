import { useNavigate } from "@tanstack/react-router";
import { CheckIcon, CrownIcon, DownloadIcon, FileJsonIcon, GamepadIcon, Trash2Icon, TriangleAlertIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState, type FormEvent } from "react";
import { deleteAccount, exportData, getTwoFactor, run } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useInstance } from "@/fuwa/hooks";
import { ServerIcon } from "@/components/Icons";
import { SPRING } from "@/components/motion";
import { Private } from "@/components/Private";
import { hasPassword } from "@/components/settings/Account";
import { PasswordInput, useShake } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { formatBytes } from "@/lib/format";
import { Switch } from "@/components/ui/switch";
import { savePresenceSettings, usePresenceSettings } from "@/fuwa/presence";
import { engine } from "@/fuwa/sync";
import { closeSettings, toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/**
 * What the instance keeps about you: download all of it as one file, or
 * delete the account. Messages stay in their servers, from "Deleted
 * account", so conversations still read.
 */
export function Privacy({ instanceKey }: { instanceKey: string }) {
  const inst = useInstance(instanceKey);
  const where = inst?.node?.name ?? "this instance";
  const [deleting, setDeleting] = useState(false);
  if (!inst?.me) return null;
  const owned = inst.servers.filter((s) => s.ownerId === inst.me!.id);

  return (
    <div className="flex flex-col gap-8">
      <ActivitySharing instanceKey={instanceKey} where={where} />
      <Export instanceKey={instanceKey} where={where} />

      <motion.section
        data-setting="delete-account"
        initial={{ opacity: 0, y: 12 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ ...SPRING, delay: 0.06 }}
        className="rounded-3xl border border-destructive/30 bg-destructive/5 p-5"
      >
        <div className="flex flex-col gap-4 sm:flex-row sm:items-center">
          <span className="grid size-12 shrink-0 place-items-center rounded-2xl bg-destructive/15 text-destructive">
            <Trash2Icon className="size-6" />
          </span>
          <div className="min-w-0 flex-1">
            <p className="font-extrabold">Delete your account</p>
            <p className="text-sm text-muted-foreground">
              Your account on {where} goes for good: profile, settings and devices. Messages you sent stay in their servers, from "Deleted account".
            </p>
          </div>
          <Button type="button" variant="destructive" className="shrink-0 rounded-xl font-bold" onClick={() => setDeleting(true)} disabled={owned.length > 0}>
            Delete account
          </Button>
        </div>
        <AnimatePresence initial={false}>
          {owned.length > 0 && (
            <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} className="overflow-hidden">
              <div className="mt-4 rounded-2xl bg-background/60 p-3">
                <p className="mb-2 flex items-center gap-1.5 text-sm font-bold">
                  <CrownIcon className="size-4 text-amber-400" /> You own {owned.length === 1 ? "a server" : `${owned.length} servers`}. Delete {owned.length === 1 ? "it" : "them"} first.
                </p>
                <ul className="flex flex-wrap gap-2">
                  {owned.map((s) => (
                    <li key={s.id} className="flex items-center gap-2 rounded-xl border bg-card py-1 pr-3 pl-1 text-sm font-bold">
                      <ServerIcon server={s} className="size-6 rounded-lg text-[0.6rem]" />
                      {s.name}
                    </li>
                  ))}
                </ul>
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </motion.section>

      <DeleteDialog instanceKey={instanceKey} where={where} open={deleting} onOpenChange={setDeleting} />
    </div>
  );
}

function Export({ instanceKey, where }: { instanceKey: string; where: string }) {
  const [state, setState] = useState<"idle" | "working" | "done">("idle");
  const [bytes, setBytes] = useState(0);
  const [file, setFile] = useState<{ url: string; name: string } | null>(null);

  useEffect(() => () => void (file && URL.revokeObjectURL(file.url)), [file]);

  async function start() {
    setState("working");
    setBytes(0);
    try {
      const blob = await run(exportData(instanceKey, setBytes));
      const name = `fuwa-${where.replace(/[^\w.-]+/g, "-").toLowerCase()}-${new Date().toISOString().slice(0, 10)}.json`;
      const url = URL.createObjectURL(blob);
      setFile({ url, name });
      setBytes(blob.size);
      setState("done");
      save(url, name);
    } catch (err) {
      toast((err as FuwaError).message);
      setState("idle");
    }
  }

  function save(url: string, name: string) {
    Object.assign(document.createElement("a"), { href: url, download: name }).click();
  }

  return (
    <motion.section data-setting="export" initial={{ opacity: 0, y: 12 }} animate={{ opacity: 1, y: 0 }} transition={SPRING} className="relative overflow-hidden rounded-3xl border bg-card p-5">
      <div className="flex flex-col gap-4 sm:flex-row sm:items-center">
        <span className="relative grid size-12 shrink-0 place-items-center overflow-hidden rounded-2xl bg-primary/15 text-primary">
          <AnimatePresence mode="popLayout" initial={false}>
            {state === "done" ? (
              <motion.span key="done" initial={{ scale: 0, rotate: -45 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 500, damping: 14 }}>
                <CheckIcon className="size-6" strokeWidth={3} />
              </motion.span>
            ) : (
              <motion.span
                key="file"
                initial={{ scale: 0.5, opacity: 0 }}
                animate={state === "working" ? { y: [0, -3, 0], scale: 1, opacity: 1 } : { scale: 1, opacity: 1, y: 0 }}
                transition={state === "working" ? { y: { duration: 0.8, repeat: Infinity }, default: SPRING } : SPRING}
              >
                <FileJsonIcon className="size-6" />
              </motion.span>
            )}
          </AnimatePresence>
        </span>
        <div className="min-w-0 flex-1">
          <p className="font-extrabold">Download your data</p>
          <p className="text-sm text-muted-foreground">
            Everything {where} keeps about you, as one JSON file: your account, profile, settings, devices, servers, and every message you've sent.
          </p>
        </div>
        <AnimatePresence mode="popLayout" initial={false}>
          {state === "done" && file ? (
            <motion.div key="again" initial={{ opacity: 0, scale: 0.9 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.9 }} transition={SPRING}>
              <Button type="button" variant="outline" className="shrink-0 rounded-xl" onClick={() => save(file.url, file.name)}>
                <DownloadIcon className="size-4" /> Save again
              </Button>
            </motion.div>
          ) : (
            <motion.div key="start" initial={{ opacity: 0, scale: 0.9 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.9 }} transition={SPRING}>
              <Button type="button" className="btn shrink-0 rounded-xl px-5 font-bold" onClick={() => void start()} disabled={state === "working"}>
                <DownloadIcon className="size-4" /> {state === "working" ? "Gathering…" : "Download"}
              </Button>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
      <AnimatePresence initial={false}>
        {state !== "idle" && (
          <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} transition={SPRING} className="overflow-hidden">
            <div className="mt-4 flex items-center gap-3">
              <div className="relative h-2 flex-1 overflow-hidden rounded-full bg-muted">
                {state === "working" ? (
                  <motion.span
                    className="absolute inset-y-0 w-1/3 rounded-full bg-primary"
                    initial={{ left: "-33%" }}
                    animate={{ left: "100%" }}
                    transition={{ duration: 1.1, repeat: Infinity, ease: "easeInOut" }}
                  />
                ) : (
                  <motion.span className="absolute inset-0 origin-left rounded-full bg-emerald-500" initial={{ scaleX: 0.3 }} animate={{ scaleX: 1 }} transition={{ duration: 0.5, ease: [0.22, 1, 0.36, 1] }} />
                )}
              </div>
              <span className="w-28 text-right text-xs font-bold text-muted-foreground tabular-nums">
                {state === "done" ? `Ready, ${formatBytes(bytes)}` : formatBytes(bytes)}
              </span>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </motion.section>
  );
}

function DeleteDialog({ instanceKey, where, open, onOpenChange }: { instanceKey: string; where: string; open: boolean; onOpenChange: (open: boolean) => void }) {
  const inst = useInstance(instanceKey);
  const me = inst?.me;
  const standalone = hasPassword(me ?? undefined);
  const [twoStep, setTwoStep] = useState(false);
  const [password, setPassword] = useState("");
  const [show, setShow] = useState(false);
  const [code, setCode] = useState("");
  const [username, setUsername] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [shake, doShake] = useShake();
  const navigate = useNavigate();

  useEffect(() => {
    if (!open) return;
    setPassword("");
    setCode("");
    setUsername("");
    setError(null);
    if (standalone) run(getTwoFactor(instanceKey)).then((s) => setTwoStep(s.enabled), () => setTwoStep(false));
  }, [open, instanceKey, standalone]);

  if (!me) return null;
  const ready = standalone ? password.length > 0 && (!twoStep || code.trim().length >= 6) : username.trim().toLowerCase() === me.username;

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!ready) return doShake();
    setPending(true);
    setError(null);
    try {
      await run(deleteAccount(instanceKey, standalone ? { password, code: code.trim() } : { username: username.trim() }));
      onOpenChange(false);
      closeSettings();
      toast(`Your account on ${where} is gone`);
      void navigate({ to: "/" });
    } catch (err) {
      setError((err as FuwaError).message);
      doShake();
    } finally {
      setPending(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={(next) => !pending && onOpenChange(next)}>
      <DialogContent>
        <DialogHeader title="Delete your account?" description={`This can't be undone. Once it's gone, ${where} leaves your list on this device too.`} />
        <form onSubmit={submit} className="flex flex-col gap-4">
          <ul className="flex flex-col gap-2 rounded-2xl bg-muted/60 p-3 text-sm">
            {["Your profile, settings and devices are deleted.", 'Your messages stay, from "Deleted account".', "You leave every server you're in."].map((line, n) => (
              <motion.li key={line} initial={{ opacity: 0, x: -8 }} animate={{ opacity: 1, x: 0 }} transition={{ ...SPRING, delay: 0.1 + n * 0.05 }} className="flex gap-2">
                <TriangleAlertIcon className="mt-0.5 size-4 shrink-0 text-destructive" />
                {line}
              </motion.li>
            ))}
          </ul>
          <motion.div animate={shake} className="flex flex-col gap-3">
            {standalone ? (
              <>
                <label className="flex flex-col gap-1.5">
                  <span className="text-xs font-bold text-muted-foreground">Your password</span>
                  <PasswordInput id="delete-password" autoComplete="current-password" value={password} onChange={setPassword} show={show} onShow={setShow} />
                </label>
                <AnimatePresence initial={false}>
                  {twoStep && (
                    <motion.label initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} className="flex flex-col gap-1.5 overflow-hidden">
                      <span className="text-xs font-bold text-muted-foreground">A code from your app, or a backup code</span>
                      <Input value={code} onChange={(e) => setCode(e.target.value)} autoComplete="one-time-code" maxLength={16} spellCheck={false} placeholder="123456" className="h-11 rounded-xl font-mono tracking-wider" />
                    </motion.label>
                  )}
                </AnimatePresence>
              </>
            ) : (
              <label className="flex flex-col gap-1.5">
                <span className="text-xs font-bold text-muted-foreground">
                  Type your username, <Private text={me.username} kind="name" className="font-mono text-foreground" />, to confirm
                </span>
                <Input value={username} onChange={(e) => setUsername(e.target.value)} autoComplete="off" spellCheck={false} className="h-11 rounded-xl" />
              </label>
            )}
          </motion.div>
          <AnimatePresence initial={false}>
            {error && (
              <motion.p initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} className="overflow-hidden text-sm font-bold text-destructive first-letter:uppercase">
                {error}
              </motion.p>
            )}
          </AnimatePresence>
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" className="rounded-xl" onClick={() => onOpenChange(false)} disabled={pending}>
              Keep my account
            </Button>
            <Button type="submit" variant="destructive" className={cn("rounded-xl font-bold transition-opacity", !ready && "opacity-60")} disabled={pending}>
              {pending ? "Deleting…" : "Delete forever"}
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/**
 * "Show what I'm doing": whether people who share a server with you see
 * the games and apps your desktop app picks up, and in which servers. Off
 * until you turn it on; your status dot shows either way.
 */
function ActivitySharing({ instanceKey, where }: { instanceKey: string; where: string }) {
  const inst = useInstance(instanceKey);
  const settings = usePresenceSettings(instanceKey);
  const allowed = inst?.node?.richPresence ?? false;
  if (!settings || !inst) return null;
  const hidden = new Set(settings.hiddenServerIds);
  const save = (change: Parameters<typeof savePresenceSettings>[2]) =>
    savePresenceSettings(instanceKey, engine(instanceKey).api, change).catch((err: FuwaError) => toast(`Couldn't save that: ${err.message}`));
  return (
    <motion.section
      data-setting="activity-sharing"
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      transition={SPRING}
      className="rounded-3xl border bg-card p-5"
    >
      <div className="flex items-start gap-4">
        <span className="grid size-12 shrink-0 place-items-center rounded-2xl bg-primary/15 text-primary">
          <GamepadIcon className="size-6" />
        </span>
        <div className="min-w-0 flex-1">
          <p className="font-extrabold">Show what I'm doing</p>
          <p className="text-sm text-muted-foreground">
            {allowed
              ? "Games and apps the desktop app sees, shown to people who share a server with you. Nothing is kept: it's gone when you stop."
              : `${where} doesn't show what people are doing. Your status still shows.`}
          </p>
        </div>
        <Switch
          className="mt-1 shrink-0"
          checked={settings.showActivity && allowed}
          disabled={!allowed}
          aria-label="Show what I'm doing"
          onCheckedChange={(on) => void save((s) => ({ ...s, showActivity: on }))}
        />
      </div>
      <AnimatePresence initial={false}>
        {settings.showActivity && allowed && inst.servers.length > 0 && (
          <motion.div initial={{ opacity: 0, y: -8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -8 }} transition={SPRING}>
            <p className="mt-5 mb-2 text-[0.7rem] font-extrabold tracking-wide text-muted-foreground uppercase">Share my activity here</p>
            <ul className="flex flex-col gap-1">
              {inst.servers.map((server) => (
                <li key={server.id}>
                  <label className="flex cursor-pointer items-center gap-3 rounded-xl px-2 py-1.5 transition hover:bg-muted/70">
                    <ServerIcon server={server} className="size-8 rounded-xl text-xs" />
                    <span className="min-w-0 flex-1 truncate text-sm font-bold">{server.name}</span>
                    <Switch
                      checked={!hidden.has(server.id)}
                      aria-label={`Share my activity in ${server.name}`}
                      onCheckedChange={(on) =>
                        void save((s) => ({
                          ...s,
                          hiddenServerIds: on ? s.hiddenServerIds.filter((id) => id !== server.id) : [...s.hiddenServerIds, server.id],
                        }))
                      }
                    />
                  </label>
                </li>
              ))}
            </ul>
          </motion.div>
        )}
      </AnimatePresence>
    </motion.section>
  );
}
