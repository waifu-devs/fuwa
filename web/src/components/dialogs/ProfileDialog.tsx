import { CheckIcon, LoaderCircleIcon, LogOutIcon } from "lucide-react";
import { motion } from "motion/react";
import { useEffect, useState, type FormEvent } from "react";
import { forget, signOut, updateProfile } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { UserAvatar } from "@/components/Icons";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Tabs, TabsContent, TabsContents, TabsList, TabsTrigger } from "@/components/ui/tabs";
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
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent wide>
        <DialogHeader title="Settings" description={inst?.node ? `Your account on ${inst.node.name} (${instanceKey})` : undefined} />
        <Tabs defaultValue="profile">
          <TabsList className="w-full">
            <TabsTrigger value="profile">Profile</TabsTrigger>
            <TabsTrigger value="look">Appearance</TabsTrigger>
          </TabsList>
          <TabsContents className="pt-4">
            <TabsContent value="profile">{open && <Profile instanceKey={instanceKey} onDone={() => onOpenChange(false)} />}</TabsContent>
            <TabsContent value="look">
              <Themes />
            </TabsContent>
          </TabsContents>
        </Tabs>
      </DialogContent>
    </Dialog>
  );
}

function Profile({ instanceKey, onDone }: { instanceKey: string; onDone: () => void }) {
  const inst = useInstance(instanceKey);
  const me = inst?.me;
  const [name, setName] = useState(me?.displayName ?? "");
  const [avatar, setAvatar] = useState(me?.avatarUrl ?? "");
  const save = useAction(updateProfile);
  const leave = useAction(signOut);
  const drop = useAction(forget);
  if (!me) return null;
  const preview = { ...me, displayName: name, avatarUrl: avatar };

  async function submit(e: FormEvent) {
    e.preventDefault();
    const ok = await save.go(instanceKey, name.trim(), avatar.trim());
    if (ok !== undefined) onDone();
  }

  return (
    <div className="flex flex-col gap-5">
      <form onSubmit={submit} className="flex flex-col gap-4">
        <div className="flex items-center gap-4 rounded-3xl border bg-background/50 p-4">
          <span className="avatar-ring rounded-full p-[3px]">
            <UserAvatar user={preview} className="size-16 text-2xl ring-4 ring-card" />
          </span>
          <div className="min-w-0">
            <p className="truncate text-lg font-extrabold">{name || me.username}</p>
            <p className="truncate text-sm text-muted-foreground">@{me.username}</p>
          </div>
        </div>
        <div className="grid gap-4 sm:grid-cols-2">
          <div className="flex flex-col gap-2">
            <Label htmlFor="profile-name" className="font-bold">
              Display name
            </Label>
            <Input id="profile-name" maxLength={64} value={name} onChange={(e) => setName(e.target.value)} className="h-11 rounded-xl" />
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor="profile-avatar" className="font-bold">
              Avatar URL
            </Label>
            <Input id="profile-avatar" type="url" placeholder="https://…" value={avatar} onChange={(e) => setAvatar(e.target.value)} className="h-11 rounded-xl" />
          </div>
        </div>
        {save.error && <p className="text-sm text-destructive first-letter:uppercase">{save.error}</p>}
        <Button type="submit" disabled={save.pending} className="btn h-11 self-end rounded-xl px-6 font-bold">
          {save.pending && <LoaderCircleIcon className="animate-spin" />}
          Save profile
        </Button>
      </form>
      <div className="flex flex-wrap gap-2 border-t pt-4">
        <Button variant="outline" className="rounded-xl" disabled={leave.pending} onClick={() => leave.go(instanceKey)}>
          <LogOutIcon /> Sign out of {inst?.node?.name ?? instanceKey}
        </Button>
        <Button variant="ghost" className="rounded-xl text-destructive hover:text-destructive" disabled={drop.pending} onClick={() => drop.go(instanceKey)}>
          Remove from this browser
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
