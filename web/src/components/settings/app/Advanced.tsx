import { BugIcon, CheckIcon, FingerprintIcon, GaugeIcon, MousePointerClickIcon, ServerIcon, XIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { Count, SPRING } from "@/components/motion";
import { Toggle } from "@/components/settings/controls";
import { useFuwa } from "@/fuwa/store";
import { setPrefs, usePrefs } from "@/lib/prefs";
import { usePendingReport } from "@/lib/reports";
import { cn } from "@/lib/utils";
import { PrefSetting } from "./common";

export function Advanced() {
  const on = usePrefs((p) => p.developerMode);
  return (
    <div className="flex flex-col">
      <ShareReports />
      <PrefSetting id="developer-mode" title="Developer mode" keys={["developerMode"]}>
        <Toggle
          checked={on}
          onChange={(developerMode) => setPrefs({ developerMode })}
          label="Show Copy ID on servers, channels, people and messages"
          hint="Handy for bots, the API and bug reports."
        />
        <div className="flex items-center gap-2 rounded-xl bg-muted/50 px-3 py-2.5 text-sm">
          <span className="min-w-0 flex-1 truncate">
            <b>#general</b> <span className="text-muted-foreground">· the channel's header</span>
          </span>
          <AnimatePresence initial={false}>
            {on && (
              <motion.span
                initial={{ opacity: 0, scale: 0.6, x: 8 }}
                animate={{ opacity: 1, scale: 1, x: 0 }}
                exit={{ opacity: 0, scale: 0.6, x: 8 }}
                transition={SPRING}
                className="flex shrink-0 items-center gap-1.5 rounded-lg bg-background px-2 py-1 text-xs font-bold shadow-sm"
              >
                <FingerprintIcon className="size-3.5 text-primary" /> Copy channel ID
              </motion.span>
            )}
          </AnimatePresence>
        </div>
      </PrefSetting>
    </div>
  );
}

const SENT = [
  "Kinds of errors, and where in the app they happened",
  "How long things took: starting up, catching up, calls connecting, requests, slow frames",
  "How often a few features are used, like sending a message or sharing your screen",
  "This app's version, your browser family and OS family",
];
const NEVER = ["Messages or anything you type", "Names, file names, ids or links", "Your address, or anything that tells you apart"];

/** Anonymous reports: what's sent, where it goes, and what's waiting to go. */
function ShareReports() {
  const on = usePrefs((p) => p.shareReports);
  const pending = usePendingReport();
  // Where reports go: the first instance you're signed in to that takes them (see reportTarget).
  const target = useFuwa((s) => {
    const key = s.order.find((k) => s.instances[k]?.me && s.instances[k]?.node?.telemetry);
    return key ? s.instances[key]!.node!.name || key : null;
  });
  const counts = [
    { icon: BugIcon, label: "error", value: pending.errors },
    { icon: GaugeIcon, label: "timing", value: pending.timings },
    { icon: MousePointerClickIcon, label: "feature", value: pending.usage },
  ];
  return (
    <PrefSetting id="share-reports" title="Help fix bugs" keys={["shareReports"]}>
      <Toggle
        checked={on}
        onChange={(shareReports) => setPrefs({ shareReports })}
        label="Send anonymous reports of errors and slow spots"
        hint="Counts only, so we can find and fix what breaks or drags."
      />
      <motion.div
        animate={{ opacity: on ? 1 : 0.55 }}
        transition={SPRING}
        className="grid gap-2 text-sm sm:grid-cols-2"
      >
        <ul className="flex flex-col gap-1.5 rounded-xl bg-muted/50 px-3 py-2.5">
          <li className="text-xs font-bold uppercase tracking-wide text-muted-foreground">What's sent</li>
          {SENT.map((line, n) => (
            <motion.li
              key={line}
              initial={{ opacity: 0, x: -6 }}
              animate={{ opacity: 1, x: 0 }}
              transition={{ ...SPRING, delay: 0.04 * n }}
              className="flex gap-2"
            >
              <CheckIcon className="mt-0.5 size-3.5 shrink-0 text-primary" />
              <span>{line}</span>
            </motion.li>
          ))}
        </ul>
        <ul className="flex flex-col gap-1.5 rounded-xl bg-muted/50 px-3 py-2.5">
          <li className="text-xs font-bold uppercase tracking-wide text-muted-foreground">Never sent</li>
          {NEVER.map((line, n) => (
            <motion.li
              key={line}
              initial={{ opacity: 0, x: -6 }}
              animate={{ opacity: 1, x: 0 }}
              transition={{ ...SPRING, delay: 0.04 * (n + SENT.length) }}
              className="flex gap-2"
            >
              <XIcon className="mt-0.5 size-3.5 shrink-0 text-destructive" />
              <span>{line}</span>
            </motion.li>
          ))}
        </ul>
      </motion.div>
      <AnimatePresence initial={false}>
        {on && (
          <motion.div
            key="waiting"
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            transition={SPRING}
            className="overflow-hidden"
          >
            <div className="flex flex-col gap-2 rounded-xl border border-border/60 bg-background/60 px-3 py-2.5 text-sm">
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-xs font-bold uppercase tracking-wide text-muted-foreground">Waiting to send</span>
                {counts.map(({ icon: Icon, label, value }) => (
                  <motion.span
                    key={label}
                    animate={{ scale: value > 0 ? [1, 1.08, 1] : 1 }}
                    transition={{ duration: 0.35 }}
                    className={cn(
                      "flex items-center gap-1.5 rounded-lg px-2 py-1 text-xs font-bold",
                      value > 0 ? "bg-primary/12 text-primary" : "bg-muted text-muted-foreground",
                    )}
                  >
                    <Icon className="size-3.5" />
                    <Count value={value} /> {value === 1 ? label : `${label}s`}
                  </motion.span>
                ))}
              </div>
              <p className="flex items-start gap-2 text-xs text-muted-foreground">
                <ServerIcon className="mt-0.5 size-3.5 shrink-0" />
                {target ? (
                  <span>
                    Goes only to <b className="text-foreground">{target}</b>, which adds it to its hourly report to Waifu
                    Devs. Instances whose owners turned telemetry off get nothing.
                  </span>
                ) : (
                  <span>
                    None of your instances take reports right now (their owners turned telemetry off, or you're signed
                    out), so nothing leaves this browser.
                  </span>
                )}
              </p>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </PrefSetting>
  );
}
