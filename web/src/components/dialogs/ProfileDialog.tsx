import { CheckIcon, LogOutIcon, PaletteIcon, UserRoundIcon } from "lucide-react";
import { motion } from "motion/react";
import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import type { User } from "@/gen/fuwa/v1/types_pb";
import { forget, signOut, updateProfile } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { hue, UserAvatar } from "@/components/Icons";
import { SwapText } from "@/components/motion";
import { SaveBar, WithPreview } from "@/components/settings/controls";
import { SettingsScreen } from "@/components/settings/SettingsScreen";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { applyTheme, BUILTIN_THEMES, savedTheme, type Theme } from "@/lib/themes";
import { cn } from "@/lib/utils";

export function ProfileDialog({
  open,
  onOpenChange,
  instanceKey,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instanceKey: string;
}) {
  const inst = useInstance(instanceKey);
  const [section, setSection] = useState("profile");
  useEffect(() => {
    if (open) setSection("profile");
  }, [open]);
  const where = inst?.node?.name ?? instanceKey;
  return (
    <SettingsScreen
      open={open}
      onOpenChange={onOpenChange}
      title="Settings"
      subtitle={`Your account on ${where}`}
      section={section}
      onSectionChange={setSection}
      groups={[
        {
          label: "Your account",
          sections: [
            { id: "profile", label: "Profile", icon: UserRoundIcon, description: `How people see you on ${where} (${instanceKey}).` },
            { id: "look", label: "Appearance", icon: PaletteIcon, description: "The theme for this app, on every fuwa server you use here." },
          ],
        },
        { sections: [{ id: "session", label: "Sign out", icon: LogOutIcon, danger: true }] },
      ]}
    >
      {section === "profile" && <Profile instanceKey={instanceKey} />}
      {section === "look" && <Themes />}
      {section === "session" && <Session instanceKey={instanceKey} />}
    </SettingsScreen>
  );
}

function Profile({ instanceKey }: { instanceKey: string }) {
  const inst = useInstance(instanceKey);
  const me = inst?.me;
  const [name, setName] = useState(me?.displayName ?? "");
  const [avatar, setAvatar] = useState(me?.avatarUrl ?? "");
  const save = useAction(updateProfile);
  if (!me) return null;
  const preview = { ...me, displayName: name, avatarUrl: avatar };
  const changes = [name !== me.displayName, avatar !== me.avatarUrl].filter(Boolean).length;

  async function submit(e?: FormEvent) {
    e?.preventDefault();
    await save.go(instanceKey, name.trim(), avatar.trim());
  }

  return (
    <form onSubmit={submit}>
      <WithPreview preview={<ProfileCard user={preview} />}>
        <div className="flex flex-col">
          <Row label="Display name" htmlFor="profile-name" hint="What people see next to your messages.">
            <Input id="profile-name" maxLength={64} value={name} placeholder={me.username} onChange={(e) => setName(e.target.value)} className="h-11 rounded-xl" />
          </Row>
          <Row label="Avatar" htmlFor="profile-avatar" hint="A link to a picture. Without one you get your initial on your own color.">
            <Input id="profile-avatar" type="url" placeholder="https://…" value={avatar} onChange={(e) => setAvatar(e.target.value)} className="h-11 rounded-xl" />
          </Row>
          <Row label="Username" hint="Set when the account was made.">
            <p className="text-sm font-bold">@{me.username}</p>
          </Row>
        </div>
        <SaveBar
          count={changes}
          saving={save.pending}
          error={save.error}
          onSave={() => void submit()}
          onDiscard={() => {
            setName(me.displayName);
            setAvatar(me.avatarUrl);
            save.setError(null);
          }}
        />
      </WithPreview>
    </form>
  );
}

/** One field of a form, as a flat row under a rule. */
function Row({ label, htmlFor, hint, children }: { label: string; htmlFor?: string; hint?: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-2 border-b border-border/70 py-5 first:pt-0 last:border-b-0">
      <Label htmlFor={htmlFor} className="font-extrabold">
        {label}
      </Label>
      {children}
      {hint && <p className="text-sm text-muted-foreground">{hint}</p>}
    </div>
  );
}

/** How others see you: a card with your color, avatar and name, and one of your messages. */
function ProfileCard({ user }: { user: User }) {
  const shown = user.displayName || user.username;
  return (
    <div className="overflow-hidden rounded-3xl border bg-card shadow-lg">
      <motion.div key={user.avatarUrl} style={hue(user.id)} className="server-gradient h-24" initial={{ opacity: 0.6 }} animate={{ opacity: 1 }} />
      <div className="relative -mt-11 px-4 pb-4">
        <span className="avatar-ring inline-block rounded-full p-[3px]">
          <UserAvatar user={user} className="size-20 text-3xl ring-4 ring-card" />
        </span>
        <p className="mt-2 truncate text-xl font-extrabold">
          <SwapText className="truncate align-bottom">{shown}</SwapText>
        </p>
        <p className="truncate text-sm text-muted-foreground">@{user.username}</p>
        <div className="mt-4 flex gap-2.5 rounded-2xl bg-muted/60 p-3">
          <UserAvatar user={user} className="size-8 text-xs" />
          <div className="min-w-0">
            <p className="truncate text-sm font-extrabold">
              <SwapText className="truncate align-bottom">{shown}</SwapText>{" "}
              <span className="text-xs font-normal text-muted-foreground">Today</span>
            </p>
            <p className="text-sm">This is how my messages look ✨</p>
          </div>
        </div>
      </div>
    </div>
  );
}

function Session({ instanceKey }: { instanceKey: string }) {
  const inst = useInstance(instanceKey);
  const leave = useAction(signOut);
  const drop = useAction(forget);
  const where = inst?.node?.name ?? instanceKey;
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-3 rounded-2xl border p-4 sm:flex-row sm:items-center">
        <div className="min-w-0 flex-1">
          <p className="text-sm font-bold">Sign out of {where}</p>
          <p className="text-xs text-muted-foreground">It stays in your list, so signing back in is one step.</p>
        </div>
        <Button variant="outline" className="rounded-xl" disabled={leave.pending} onClick={() => leave.go(instanceKey)}>
          <LogOutIcon /> Sign out
        </Button>
      </div>
      <div className="flex flex-col gap-3 rounded-2xl border border-destructive/40 bg-destructive/5 p-4 sm:flex-row sm:items-center">
        <div className="min-w-0 flex-1">
          <p className="text-sm font-bold text-destructive">Remove from this browser</p>
          <p className="text-xs text-muted-foreground">Signs out and takes {where} off your server list here. Your account stays on the instance.</p>
        </div>
        <Button variant="destructive" className="rounded-xl" disabled={drop.pending} onClick={() => drop.go(instanceKey)}>
          Remove
        </Button>
      </div>
    </div>
  );
}

function Themes() {
  const [current, setCurrent] = useState<Theme>(savedTheme);
  useEffect(() => applyTheme(current), [current]);
  return (
    <div className="grid gap-3 sm:grid-cols-2">
      {BUILTIN_THEMES.map((theme) => {
        const t = theme.variant.tokens;
        const active = theme.id === current.id;
        return (
          <motion.button
            key={theme.id}
            type="button"
            whileHover={{ y: -3 }}
            whileTap={{ scale: 0.97 }}
            onClick={() => setCurrent(theme)}
            style={{ background: t.background, color: t.foreground, borderColor: active ? t.primary : t.border }}
            className={cn("relative flex flex-col gap-3 overflow-hidden rounded-2xl border-2 p-4 text-left", active && "shadow-lg")}
          >
            <span className="flex items-center gap-2">
              {[t.primary, t.card, t["muted-foreground"], t.border].map((c, n) => (
                <span key={n} className="size-5 rounded-full border" style={{ background: c, borderColor: t.border }} />
              ))}
            </span>
            <span>
              <span className="block font-extrabold">{theme.name}</span>
              <span className="block text-xs" style={{ color: t["muted-foreground"] }}>
                {theme.description}
              </span>
            </span>
            {active && (
              <motion.span
                layoutId="theme-check"
                className="absolute top-3 right-3 grid size-6 place-items-center rounded-full"
                style={{ background: t.primary, color: t["primary-foreground"] }}
              >
                <CheckIcon className="size-4" />
              </motion.span>
            )}
          </motion.button>
        );
      })}
    </div>
  );
}
