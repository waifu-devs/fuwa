import { ClipboardPenIcon, EyeIcon, HourglassIcon, Undo2Icon, XIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";
import { ApplicationStatus } from "@/gen/fuwa/v1/types_pb";
import { checkApplied, dismissApplied, run, withdrawApplication } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useInstances } from "@/fuwa/hooks";
import type { InstanceState } from "@/fuwa/store";
import { ServerIcon } from "@/components/Icons";
import { ApplicationDialog } from "@/components/join/ApplicationStatus";
import { ApplyDialog } from "@/components/join/ApplyDialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { Applied } from "@/lib/applied";
import { ago } from "@/lib/format";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** How often to ask whether anyone looked at your applications. */
const EVERY = 30_000;

/**
 * Asks each instance now and then how your applications went, and when you
 * come back to the tab, and says so when one lets you in or turns you down.
 * Nothing runs while you aren't waiting on any.
 */
export function AppliedWatcher() {
  const instances = useInstances();
  const waiting = instances
    .filter((i) => i.me && i.connection === "live" && Object.values(i.applied).some((a) => a.status === ApplicationStatus.PENDING))
    .map((i) => i.key)
    .join(" ");
  useEffect(() => {
    if (!waiting) return;
    const keys = waiting.split(" ");
    const check = () => {
      for (const key of keys)
        run(checkApplied(key)).then(
          ({ letIn, turnedDown }) => {
            for (const s of letIn) toast(`You're in ${s.name}! Say hi`);
            for (const s of turnedDown) toast(`${s.name} turned down your application`);
          },
          () => {
            // Asked again on the next round.
          },
        );
    };
    check();
    const id = setInterval(check, EVERY);
    const onShow = () => document.visibilityState === "visible" && check();
    document.addEventListener("visibilitychange", onShow);
    return () => {
      clearInterval(id);
      document.removeEventListener("visibilitychange", onShow);
    };
  }, [waiting]);
  return null;
}

/**
 * A server you applied to, waiting in the rail: faded, with an hourglass while
 * it's being looked at, or a cross once it's turned down. Its menu takes the
 * application back, applies again, or lets it go.
 */
export function AppliedButton({ inst, applied }: { inst: Pick<InstanceState, "key">; applied: Applied }) {
  const { server } = applied;
  const waiting = applied.status === ApplicationStatus.PENDING;
  const [applying, setApplying] = useState(false);
  const [looking, setLooking] = useState(false);
  const label = waiting ? `${server.name} · waiting to be let in` : `${server.name} · turned down`;

  function withdraw() {
    run(withdrawApplication(inst.key, server.id)).then(
      () => toast(`Took back your application to ${server.name}`),
      (e: FuwaError) => toast(e.message),
    );
  }

  return (
    <div className="relative flex w-full justify-center">
      <DropdownMenu>
        <Tooltip>
          <TooltipTrigger asChild>
            <DropdownMenuTrigger asChild>
              <button type="button" aria-label={label} className="group relative rounded-[50%] outline-none focus-visible:ring-2 focus-visible:ring-ring">
                <span className={cn("block transition duration-300 group-hover:opacity-100 group-hover:grayscale-0", waiting ? "opacity-60 grayscale" : "opacity-40 grayscale")}>
                  <ServerIcon server={server} active={false} />
                </span>
                <span aria-hidden className="pointer-events-none absolute inset-0 rounded-[inherit] border-2 border-dashed border-muted-foreground/40 transition group-hover:border-primary/60" />
                <AnimatePresence mode="popLayout" initial={false}>
                  <motion.span
                    key={applied.status}
                    initial={{ scale: 0, rotate: -60 }}
                    animate={{ scale: 1, rotate: 0 }}
                    exit={{ scale: 0 }}
                    transition={{ type: "spring", stiffness: 600, damping: 16 }}
                    className={cn(
                      "absolute -right-1 -bottom-1 grid size-5 place-items-center rounded-full text-white ring-[3px] ring-[color-mix(in_srgb,var(--background)_75%,black)]",
                      waiting ? "bg-amber-500" : "bg-destructive",
                    )}
                  >
                    {waiting ? <HourglassIcon className="size-3 animate-[flip_3s_ease-in-out_infinite]" /> : <XIcon className="size-3" strokeWidth={3} />}
                  </motion.span>
                </AnimatePresence>
              </button>
            </DropdownMenuTrigger>
          </TooltipTrigger>
          <TooltipContent side="right" className="font-bold">
            {label}
          </TooltipContent>
        </Tooltip>
        <DropdownMenuContent side="right" align="start" className="w-64">
          <DropdownMenuLabel className="flex flex-col gap-0.5 font-normal">
            <span className="truncate font-extrabold">{server.name}</span>
            <span className="text-xs text-muted-foreground">
              {waiting
                ? `You applied ${ago(new Date(applied.appliedAt))}. Someone from the server will look it over.`
                : applied.reason
                  ? `Turned down: “${applied.reason}”`
                  : "Your application was turned down."}
            </span>
          </DropdownMenuLabel>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={() => setLooking(true)}>
            <EyeIcon /> See where it stands
          </DropdownMenuItem>
          {waiting ? (
            <DropdownMenuItem variant="destructive" onSelect={withdraw}>
              <Undo2Icon /> Take back your application
            </DropdownMenuItem>
          ) : (
            <>
              <DropdownMenuItem onSelect={() => setApplying(true)}>
                <ClipboardPenIcon /> Apply again
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => dismissApplied(inst.key, server.id)}>
                <XIcon /> Remove from the list
              </DropdownMenuItem>
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
      <ApplyDialog open={applying} onOpenChange={setApplying} instanceKey={inst.key} server={server} inviteCode={applied.inviteCode} />
      <ApplicationDialog
        open={looking}
        onOpenChange={setLooking}
        instanceKey={inst.key}
        server={server}
        onApplyAgain={() => {
          setLooking(false);
          setApplying(true);
        }}
      />
    </div>
  );
}
