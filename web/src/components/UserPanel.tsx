import { SettingsIcon } from "lucide-react";
import { useState } from "react";
import { useInstance } from "@/fuwa/hooks";
import { ProfileDialog } from "@/components/dialogs/ProfileDialog";
import { ConnDot, UserAvatar, connectionLabel } from "@/components/Icons";
import { SwapText } from "@/components/motion";
import { displayName } from "@/lib/format";

/** You, on this instance, at the bottom of the sidebar. */
export function UserPanel({ instanceKey }: { instanceKey: string }) {
  const inst = useInstance(instanceKey);
  const [open, setOpen] = useState(false);
  if (!inst?.me) return null;
  return (
    <div className="flex items-center gap-2 border-t bg-[color-mix(in_srgb,var(--background)_50%,transparent)] p-2">
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="group flex min-w-0 flex-1 items-center gap-2 rounded-xl p-1.5 text-left transition hover:bg-muted"
      >
        <span className="relative shrink-0">
          <UserAvatar user={inst.me} className="size-9 transition duration-300 ease-[cubic-bezier(0.3,1.6,0.5,1)] group-hover:-rotate-6 group-hover:scale-110" />
          <ConnDot state={inst.connection} className="absolute -right-0.5 -bottom-0.5 ring-[3px] ring-card" />
        </span>
        <span className="min-w-0">
          <span className="block truncate text-sm font-bold">
            <SwapText className="truncate align-bottom">{displayName(inst.me)}</SwapText>
          </span>
          <span className="block truncate text-xs text-muted-foreground">
            <SwapText className="truncate align-bottom">
              {inst.connection === "live" ? `@${inst.me.username}` : connectionLabel(inst.connection)}
            </SwapText>
          </span>
        </span>
      </button>
      <button
        type="button"
        onClick={() => setOpen(true)}
        aria-label="Settings"
        className="grid size-9 place-items-center rounded-xl text-muted-foreground transition hover:bg-muted hover:text-foreground"
      >
        <SettingsIcon className="size-[18px] transition-transform duration-500 hover:rotate-180" />
      </button>
      <ProfileDialog open={open} onOpenChange={setOpen} instanceKey={instanceKey} />
    </div>
  );
}
