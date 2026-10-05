import { EyeIcon, EyeOffIcon, PencilIcon, SettingsIcon, XIcon } from "lucide-react";
import { useState } from "react";
import { useFuwa } from "@/fuwa/store";
import type { User } from "@/gen/fuwa/v1/types_pb";
import { run, updateProfile } from "@/fuwa/actions";
import { PresenceStatus } from "@/gen/fuwa/v1/presence_pb";
import { savePresenceSettings, usePresence, usePresenceSettings } from "@/fuwa/presence";
import { engine } from "@/fuwa/sync";
import { ConnDot, UserAvatar } from "@/components/Icons";
import { connectionLabel } from "@/components/icons-utils";
import { MuteButtons } from "@/components/calls/parts";
import { SwapText } from "@/components/motion";
import { AccountItems, AddAccountDialog } from "@/components/AccountSwitcher";
import { Private } from "@/components/Private";
import { StatusDot } from "@/components/Presence";
import { STATUS_LABEL, shownOf } from "@/components/presence-status";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { displayName, shownStatus } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { openSettings, toast } from "@/lib/ui";
import type { I18n } from "@/i18n/i18n";
import { useI18n } from "@/i18n/react";

const CHOICES = [
  { status: PresenceStatus.ONLINE, dot: "online", hint: "" },
  { status: PresenceStatus.IDLE, dot: "idle", hint: "workspace.userPanel.hint.idle" },
  { status: PresenceStatus.DO_NOT_DISTURB, dot: "dnd", hint: "workspace.userPanel.hint.dnd" },
  { status: PresenceStatus.INVISIBLE, dot: "offline", hint: "workspace.userPanel.hint.invisible" },
] as const;

/** Your avatar with your dot, your name, and your custom status or status under it. */
function Who({ instanceKey, me }: { instanceKey: string; me: User }) {
  const { t } = useI18n();
  const connection = useFuwa((s) => s.instances[instanceKey]?.connection ?? "connecting");
  const settings = usePresenceSettings(instanceKey);
  const own = usePresence(instanceKey, me.id);
  const now = useNow(60_000);
  const status = shownStatus(me, now);
  // Your dot follows what you picked at once; idle also comes from the instance.
  const picked = settings?.status ?? PresenceStatus.ONLINE;
  const dot = picked === PresenceStatus.ONLINE && own ? shownOf(own) : (CHOICES.find((c) => c.status === picked)?.dot ?? "online");
  const live = connection === "live";
  const subtitle = !live ? connectionLabel(connection) : status || (settings && picked !== PresenceStatus.ONLINE ? choiceLabel(t, picked) : "");
  return (
    <>
      <span className="relative shrink-0">
        <UserAvatar user={me} className="size-8 transition duration-300 ease-[cubic-bezier(0.3,1.6,0.5,1)] group-hover:-rotate-6 group-hover:scale-110" />
        {live && settings ? (
          <StatusDot status={dot} className="absolute -right-0.5 -bottom-0.5 ring-[3px] ring-card" />
        ) : (
          <ConnDot state={connection} className="absolute -right-0.5 -bottom-0.5 ring-[3px] ring-card" />
        )}
      </span>
      <span className="min-w-0">
        <span className="block truncate text-sm font-bold">
          <SwapText className="truncate align-bottom">{displayName(me)}</SwapText>
        </span>
        <span className="block truncate text-xs text-muted-foreground">
          {subtitle ? (
            <SwapText className="truncate align-bottom">{subtitle}</SwapText>
          ) : (
            <>
              @<Private text={me.username} kind="name" />
            </>
          )}
        </span>
      </span>
    </>
  );
}

const choiceLabel = (t: I18n["t"], status: PresenceStatus) => {
  const choice = CHOICES.find((c) => c.status === status);
  if (!choice) return "";
  return t(STATUS_LABEL[status === PresenceStatus.INVISIBLE ? "invisible" : choice.dot]);
};

/** You, on this instance, at the bottom of the sidebar. Your name opens your status and accounts menu. */
export function UserPanel({ instanceKey }: { instanceKey: string }) {
  const { t } = useI18n();
  const me = useFuwa((s) => s.instances[instanceKey]?.me);
  const settings = usePresenceSettings(instanceKey);
  const now = useNow(60_000);
  const [adding, setAdding] = useState(false);
  if (!me) return null;
  const status = shownStatus(me, now);
  const picked = settings?.status ?? PresenceStatus.ONLINE;
  const who = <Who instanceKey={instanceKey} me={me} />;

  const pick = (next: PresenceStatus) => {
    if (next === picked) return;
    savePresenceSettings(instanceKey, engine(instanceKey).api, (s) => ({ ...s, status: next })).catch((err) =>
      toast(t("workspace.userPanel.statusFailed", { error: err.message })),
    );
  };

  return (
    <div className="flex items-center gap-0.5 border-t bg-[color-mix(in_srgb,var(--background)_50%,transparent)] p-2">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label={t("workspace.userPanel.yourStatus")}
            className="group flex min-w-0 flex-1 items-center gap-2 rounded-xl p-1 text-left transition hover:bg-muted data-[state=open]:bg-muted"
          >
            {/* A new key when you switch accounts, so the other one lifts in. */}
            <span key={me.id} className="swap-in flex min-w-0 items-center gap-2">
              {who}
            </span>
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent side="top" align="start" className="w-64">
          {settings && (
            <>
              {CHOICES.map((c) => (
                <DropdownMenuItem key={c.status} onSelect={() => pick(c.status)} className="items-start gap-2.5 py-2">
                  <StatusDot status={c.dot} className="mt-1 shrink-0" />
                  <span className="min-w-0">
                    <span className="block font-bold">{choiceLabel(t, c.status)}</span>
                    {c.hint && <span className="block text-xs text-muted-foreground">{t(c.hint)}</span>}
                  </span>
                  {picked === c.status && <span className="ml-auto size-1.5 self-center rounded-full bg-primary" aria-label={t("workspace.userPanel.picked")} />}
                </DropdownMenuItem>
              ))}
              <DropdownMenuSeparator />
            </>
          )}
          <DropdownMenuItem onSelect={() => openSettings("profile")}>
            <PencilIcon /> {status ? t("workspace.userPanel.editStatus") : t("workspace.userPanel.setStatus")}
          </DropdownMenuItem>
          {status && (
            <DropdownMenuItem
              onSelect={() => void run(updateProfile(instanceKey, { status: "", statusExpiresAt: null })).catch(() => toast(t("workspace.userPanel.clearFailed")))}
            >
              <XIcon /> {t("workspace.userPanel.clearStatus")}
            </DropdownMenuItem>
          )}
          {settings && (
            <DropdownMenuItem onSelect={() => openSettings("privacy")}>
              {settings.showActivity ? <EyeIcon /> : <EyeOffIcon />}
              {settings.showActivity ? t("workspace.userPanel.sharing") : t("workspace.userPanel.notSharing")}
            </DropdownMenuItem>
          )}
          <AccountItems instanceKey={instanceKey} onAdd={() => setAdding(true)} />
        </DropdownMenuContent>
      </DropdownMenu>
      <AddAccountDialog instanceKey={instanceKey} open={adding} onOpenChange={setAdding} />
      <MuteButtons />
      <button
        type="button"
        onClick={() => openSettings()}
        aria-label={t("workspace.userPanel.settings")}
        className="group grid size-8 place-items-center rounded-lg text-muted-foreground transition hover:bg-muted hover:text-foreground"
      >
        <SettingsIcon className="size-[18px] transition-transform duration-500 group-hover:rotate-180" />
      </button>
    </div>
  );
}
