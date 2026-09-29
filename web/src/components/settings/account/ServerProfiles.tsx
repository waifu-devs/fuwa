import { ServerIcon as ServerGlyph } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState, type FormEvent } from "react";
import { setNickname } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { ServerIcon } from "@/components/Icons";
import { SPRING, SwapText } from "@/components/motion";
import { ProfileCard } from "@/components/ProfileCard";
import { Row } from "@/components/settings/account/common";
import { SaveBar, WithPreview } from "@/components/settings/controls";
import { Input } from "@/components/ui/input";
import { displayName } from "@/lib/format";
import { useUi } from "@/lib/ui";
import { cn } from "@/lib/utils";

const NICKNAME_MAX = 32;

/**
 * A different name in each server: pick a server, give yourself a nickname
 * there, and see the card people in that server will open.
 */
export function ServerProfiles({ instanceKey }: { instanceKey: string }) {
  const inst = useInstance(instanceKey);
  const target = useUi((u) => u.settingsTarget);
  const servers = inst?.servers ?? [];
  const [serverId, setServerId] = useState(() => (target && servers.some((s) => s.id === target) ? target : servers[0]?.id ?? ""));
  const me = inst?.me;
  const member = useFuwa((s) => s.instances[instanceKey]?.members[serverId]?.find((m) => m.user?.id === me?.id));
  const profile = useFuwa((s) => (me ? s.instances[instanceKey]?.profiles[me.id] : undefined));
  const [nickname, setDraft] = useState<string | null>(null);
  const save = useAction(setNickname);

  // A server picked from its menu opens here; one you left falls back to the first.
  useEffect(() => {
    if (target && servers.some((s) => s.id === target)) setServerId(target);
  }, [target, servers]);
  useEffect(() => {
    if (serverId && !servers.some((s) => s.id === serverId)) setServerId(servers[0]?.id ?? "");
  }, [serverId, servers]);

  if (!me) return null;
  if (!servers.length) {
    return (
      <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={SPRING} className="flex flex-col items-center gap-2 rounded-3xl border border-dashed px-6 py-12 text-center">
        <ServerGlyph className="size-8 text-muted-foreground" />
        <p className="font-extrabold">No servers yet</p>
        <p className="max-w-sm text-sm text-muted-foreground">Once you join a server you can have a different name in it here.</p>
      </motion.div>
    );
  }

  const saved = member?.nickname ?? "";
  const value = nickname ?? saved;
  const changed = nickname !== null && nickname.trim() !== saved ? 1 : 0;
  const server = servers.find((s) => s.id === serverId);

  function pick(id: string) {
    if (id === serverId) return;
    setServerId(id);
    setDraft(null);
    save.setError(null);
  }

  async function submit(e?: FormEvent) {
    e?.preventDefault();
    if (!changed) return;
    if (await save.go(instanceKey, serverId, value.trim())) setDraft(null);
  }

  return (
    <form onSubmit={submit}>
      <WithPreview preview={<ProfileCard editing me user={me} profile={profile} member={member ? { ...member, nickname: value.trim() } : undefined} />}>
        <div className="flex flex-col">
          <Row id="server" label="Server">
            <div role="radiogroup" aria-label="Server" className="flex flex-wrap gap-2">
              {servers.map((s) => {
                const active = s.id === serverId;
                return (
                  <motion.button
                    key={s.id}
                    type="button"
                    role="radio"
                    aria-checked={active}
                    disabled={changed > 0 && !active}
                    title={changed > 0 && !active ? "Save or discard first" : s.name}
                    whileHover={{ y: -2 }}
                    whileTap={{ scale: 0.95 }}
                    onClick={() => pick(s.id)}
                    className={cn(
                      "relative flex max-w-full items-center gap-2 rounded-2xl border py-1.5 pr-3 pl-1.5 text-sm font-bold transition-colors disabled:opacity-50",
                      active ? "border-primary/60" : "text-muted-foreground hover:border-primary/30 hover:text-foreground",
                    )}
                  >
                    {active && <motion.span layoutId="server-profile-pick" transition={SPRING} className="absolute inset-0 rounded-2xl bg-primary/10 ring-2 ring-primary/50" />}
                    <ServerIcon server={s} className="relative size-7 rounded-lg text-[0.65rem]" />
                    <span className="relative truncate">{s.name}</span>
                  </motion.button>
                );
              })}
            </div>
          </Row>
          <Row
            id="nickname"
            label="Nickname"
            htmlFor="server-nickname"
            hint={
              <>
                Only in <SwapText className="font-bold text-foreground">{server?.name ?? "this server"}</SwapText>. Leave it empty to go by {displayName(me)}.
              </>
            }
          >
            <AnimatePresence mode="wait" initial={false}>
              <motion.div key={serverId} initial={{ opacity: 0, x: 12 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -12 }} transition={{ duration: 0.15 }}>
                <Input
                  id="server-nickname"
                  maxLength={NICKNAME_MAX}
                  value={value}
                  placeholder={displayName(me)}
                  disabled={!member}
                  onChange={(e) => {
                    setDraft(e.target.value);
                    save.setError(null);
                  }}
                  className="h-11 max-w-sm rounded-xl"
                />
              </motion.div>
            </AnimatePresence>
          </Row>
        </div>
        <SaveBar
          count={changed}
          saving={save.pending}
          error={save.error}
          onSave={() => void submit()}
          onDiscard={() => {
            setDraft(null);
            save.setError(null);
          }}
        />
      </WithPreview>
    </form>
  );
}
