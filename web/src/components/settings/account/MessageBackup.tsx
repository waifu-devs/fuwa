import { CheckIcon, CopyIcon, DownloadIcon, HistoryIcon, KeyRoundIcon, LockKeyholeIcon, RotateCcwIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useState, type FormEvent, type ReactNode } from "react";
import { SPRING } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { dmEngine } from "@/e2ee/engine";
import { toFuwaError } from "@/fuwa/errors";
import { useFuwa } from "@/fuwa/store";
import { useI18n } from "@/i18n/react";
import { activeAgo } from "@/lib/devices";
import { formatBytes } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

const problemOf = (err: unknown) => (err instanceof Error && err.name === "Error" ? err.message : toFuwaError(err).message);

/**
 * The account's message backup: what this browser reads in direct messages
 * and secure channels, encrypted with a recovery key only you hold, so a new
 * device or browser can read what came before it.
 */
export function MessageBackup({ instanceKey }: { instanceKey: string }) {
  const { t } = useI18n();
  const backup = useFuwa((s) => s.instances[instanceKey]?.dms.backup);
  const dmsReady = useFuwa((s) => s.instances[instanceKey]?.dms.status === "ready");
  const [recoveryKey, setRecoveryKey] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [asking, setAsking] = useState<"replace" | "off" | null>(null);
  const now = useNow(60_000);

  if (!backup || backup.status === "unsupported" || (!dmsReady && backup.status === "unknown")) return null;
  const engine = dmEngine(instanceKey);

  async function act(fn: () => Promise<void>) {
    setBusy(true);
    try {
      await fn();
    } catch (err) {
      toast(problemOf(err));
    } finally {
      setBusy(false);
      setAsking(null);
    }
  }

  const create = (replace: boolean) =>
    act(async () => {
      if (!engine) return;
      setRecoveryKey(await engine.backup.create(replace));
    });

  let body: ReactNode;
  if (recoveryKey) {
    body = <KeyPanel recoveryKey={recoveryKey} onDone={() => setRecoveryKey(null)} />;
  } else if (backup.status === "unknown") {
    body = <div className="shimmer h-24 rounded-2xl" />;
  } else if (backup.status === "off") {
    body = (
      <Card icon={<HistoryIcon className="size-5" />} title={t("accountsettings.backup.off")} tone="muted">
        <p className="text-sm text-muted-foreground">{t("accountsettings.backup.offHint")}</p>
        <div className="mt-3">
          <Button type="button" size="sm" className="rounded-xl font-bold" disabled={busy || !engine} onClick={() => void create(false)}>
            <KeyRoundIcon className="size-4" /> {busy ? t("accountsettings.backup.settingUp") : t("accountsettings.backup.setUp")}
          </Button>
        </div>
      </Card>
    );
  } else if (backup.status === "locked") {
    body = (
      <Card icon={<LockKeyholeIcon className="size-5" />} title={t("accountsettings.backup.locked")} tone="primary">
        <p className="text-sm text-muted-foreground">{t("accountsettings.backup.lockedHint")}</p>
        <RestoreForm busy={busy} onRestore={(text) => act(async () => engine && (await engine.backup.restore(text)))} />
        <Ask
          open={asking === "replace"}
          question={t("accountsettings.backup.lostAsk")}
          confirm={t("accountsettings.backup.startOver")}
          busy={busy}
          onCancel={() => setAsking(null)}
          onConfirm={() => void create(true)}
        >
          <button type="button" className="mt-2 text-xs font-bold text-muted-foreground underline-offset-2 hover:text-foreground hover:underline" onClick={() => setAsking("replace")}>
            {t("accountsettings.backup.lost")}
          </button>
        </Ask>
      </Card>
    );
  } else if (backup.status === "restoring") {
    const share = backup.total ? Math.min(1, backup.restored / backup.total) : 0;
    body = (
      <Card icon={<HistoryIcon className="size-5 animate-spin [animation-duration:2.5s]" />} title={t("accountsettings.backup.restoring")} tone="primary">
        <div className="mt-1 h-2 overflow-hidden rounded-full bg-muted">
          <motion.div className="h-full origin-left rounded-full bg-primary" initial={{ scaleX: 0 }} animate={{ scaleX: share }} transition={SPRING} />
        </div>
        <p className="mt-2 text-xs text-muted-foreground tabular-nums">
          {t("accountsettings.backup.parts", { restored: backup.restored, count: backup.total })}
        </p>
      </Card>
    );
  } else {
    const share = backup.maxSize ? Math.min(1, backup.size / backup.maxSize) : 0;
    const full = backup.status === "full";
    const saved = backup.updatedAt > 0 ? activeAgo(new Date(backup.updatedAt), now) : null;
    body = (
      <Card icon={<CheckIcon className="size-5" />} title={full ? t("accountsettings.backup.full") : t("accountsettings.backup.on")} tone={full ? "warn" : "ok"}>
        <p className="text-sm text-muted-foreground">
          {t("accountsettings.backup.onHint")}
          {saved &&
            ` ${saved === "Active now" ? t("accountsettings.backup.lastSavedNow") : t("accountsettings.backup.lastSaved", { when: saved.replace(/^Active /, "") })}`}
        </p>
        <div className="mt-3 h-1.5 overflow-hidden rounded-full bg-muted" aria-hidden>
          <motion.div
            className={cn("h-full origin-left rounded-full", full ? "bg-amber-500" : "bg-emerald-500")}
            initial={false}
            animate={{ scaleX: share }}
            transition={SPRING}
          />
        </div>
        <p className="mt-1.5 text-xs text-muted-foreground tabular-nums">
          {t("accountsettings.backup.size", { used: formatBytes(backup.size), total: formatBytes(backup.maxSize) })}
        </p>
        <Ask
          open={asking !== null}
          question={
            asking === "off" ? t("accountsettings.backup.offAsk") : t("accountsettings.backup.newKeyAsk")
          }
          confirm={asking === "off" ? t("accountsettings.shared.turnOff") : t("accountsettings.backup.startOver")}
          busy={busy}
          onCancel={() => setAsking(null)}
          onConfirm={() => void (asking === "off" ? act(async () => engine && (await engine.backup.remove())) : create(true))}
        >
          <div className="mt-3 flex flex-wrap gap-2">
            <Button type="button" variant="outline" size="sm" className="rounded-xl" disabled={busy} onClick={() => setAsking("replace")}>
              <RotateCcwIcon className="size-4" /> {full ? t("accountsettings.backup.startOverHere") : t("accountsettings.backup.newKey")}
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="rounded-xl text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
              disabled={busy}
              onClick={() => setAsking("off")}
            >
              {t("accountsettings.shared.turnOff")}
            </Button>
          </div>
        </Ask>
      </Card>
    );
  }

  return (
    <section>
      <h3 className="mb-3 text-[0.7rem] font-extrabold tracking-wide text-muted-foreground uppercase">{t("accountsettings.backup.title")}</h3>
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.div
          key={recoveryKey ? "key" : backup.status === "full" ? "on" : backup.status}
          initial={{ opacity: 0, y: 10, scale: 0.98 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          exit={{ opacity: 0, y: -8, scale: 0.98 }}
          transition={SPRING}
        >
          {body}
        </motion.div>
      </AnimatePresence>
      {backup.problem && !recoveryKey && <p className="mt-2 text-xs font-bold text-amber-600 first-letter:uppercase dark:text-amber-400">{backup.problem}</p>}
    </section>
  );
}

const TONES = {
  muted: "bg-muted text-muted-foreground",
  primary: "bg-primary/15 text-primary",
  ok: "bg-emerald-500/15 text-emerald-500",
  warn: "bg-amber-500/15 text-amber-600 dark:text-amber-400",
};

function Card({ icon, title, tone, children }: { icon: ReactNode; title: string; tone: keyof typeof TONES; children: ReactNode }) {
  return (
    <div className="flex gap-3 rounded-2xl border bg-card p-4">
      <motion.span
        initial={{ scale: 0.6, rotate: -20 }}
        animate={{ scale: 1, rotate: 0 }}
        transition={{ type: "spring", stiffness: 500, damping: 16 }}
        className={cn("grid size-10 shrink-0 place-items-center rounded-xl", TONES[tone])}
      >
        {icon}
      </motion.span>
      <div className="min-w-0 flex-1">
        <p className="mb-1 text-sm font-bold">{title}</p>
        {children}
      </div>
    </div>
  );
}

/** A question asked in place of what it's about, with a cancel and a confirm. */
function Ask({
  open,
  question,
  confirm,
  busy,
  onCancel,
  onConfirm,
  children,
}: {
  open: boolean;
  question: string;
  confirm: string;
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
  children: ReactNode;
}) {
  const { t } = useI18n();
  return (
    <AnimatePresence mode="popLayout" initial={false}>
      {open ? (
        <motion.div key="ask" initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: 6 }} transition={SPRING} className="mt-3 rounded-xl border border-destructive/30 bg-destructive/5 p-3">
          <p className="text-sm">{question}</p>
          <div className="mt-2 flex gap-2">
            <Button type="button" variant="ghost" size="sm" className="rounded-xl" disabled={busy} onClick={onCancel}>
              {t("common.cancel")}
            </Button>
            <Button type="button" variant="destructive" size="sm" className="rounded-xl font-bold" disabled={busy} onClick={onConfirm}>
              {busy ? t("accountsettings.shared.working") : confirm}
            </Button>
          </div>
        </motion.div>
      ) : (
        <motion.div key="idle" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: 0.15 }}>
          {children}
        </motion.div>
      )}
    </AnimatePresence>
  );
}

function RestoreForm({ busy, onRestore }: { busy: boolean; onRestore: (text: string) => Promise<void> }) {
  const { t } = useI18n();
  const [text, setText] = useState("");
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (text.trim()) void onRestore(text);
  };
  return (
    <form onSubmit={submit} className="mt-3 flex flex-col gap-2 sm:flex-row">
      <Input
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder="XXXX-XXXX-XXXX-…"
        aria-label={t("accountsettings.backup.recoveryKey")}
        autoComplete="off"
        spellCheck={false}
        className="min-w-0 flex-1 rounded-xl font-mono text-xs tracking-wide uppercase"
      />
      <Button type="submit" size="sm" className="rounded-xl font-bold" disabled={busy || !text.trim()}>
        {busy ? t("accountsettings.backup.restoringShort") : t("accountsettings.backup.restore")}
      </Button>
    </form>
  );
}

function KeyPanel({ recoveryKey, onDone }: { recoveryKey: string; onDone: () => void }) {
  const { t } = useI18n();
  const [saved, setSaved] = useState(false);
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(recoveryKey);
      setCopied(true);
      setSaved(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      toast(t("accountsettings.backup.copyFailed"));
    }
  };
  const download = () => {
    const text = `${t("accountsettings.backup.fileTitle")}\n\n${recoveryKey}\n\n${t("accountsettings.backup.fileNote")}\n`;
    const url = URL.createObjectURL(new Blob([text], { type: "text/plain" }));
    const a = document.createElement("a");
    a.href = url;
    a.download = "fuwa-recovery-key.txt";
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    setSaved(true);
  };
  return (
    <div className="rounded-2xl border border-primary/40 bg-primary/5 p-4">
      <p className="text-sm font-bold">{t("accountsettings.backup.save")}</p>
      <p className="mt-1 text-sm text-muted-foreground">{t("accountsettings.backup.saveHint")}</p>
      <motion.code
        initial={{ opacity: 0, filter: "blur(6px)" }}
        animate={{ opacity: 1, filter: "blur(0px)" }}
        transition={{ duration: 0.5 }}
        className="mt-3 block rounded-xl border bg-background px-3 py-2.5 font-mono text-[0.8rem] leading-relaxed tracking-wider break-words select-all"
      >
        {recoveryKey}
      </motion.code>
      <div className="mt-3 flex flex-wrap gap-2">
        <Button type="button" variant="outline" size="sm" className="rounded-xl" onClick={() => void copy()}>
          {copied ? <CheckIcon className="size-4 text-emerald-500" /> : <CopyIcon className="size-4" />} {copied ? t("accountsettings.shared.copied") : t("accountsettings.shared.copy")}
        </Button>
        <Button type="button" variant="outline" size="sm" className="rounded-xl" onClick={download}>
          <DownloadIcon className="size-4" /> {t("accountsettings.shared.download")}
        </Button>
        <Button type="button" size="sm" className="ml-auto rounded-xl font-bold" disabled={!saved} onClick={onDone}>
          {t("accountsettings.shared.savedIt")}
        </Button>
      </div>
    </div>
  );
}
