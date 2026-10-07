import { ServerIcon as ServerGlyph } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useState, type FormEvent } from "react";
import type { Server } from "@/gen/fuwa/v1/types_pb";
import { updateServerProfile, type ServerProfilePatch } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { ServerIcon } from "@/components/Icons";
import { SwapText } from "@/components/motion";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { instanceHas } from "@/lib/compat";
import { ProfileCard } from "@/components/ProfileCard";
import { Row } from "@/components/settings/account/common";
import { DecorationPicker } from "@/components/settings/account/DecorationPicker";
import { EffectAbout, EffectPicker } from "@/components/settings/account/EffectPicker";
import { useDecorationsOn, useEffectsOn, useOfferedEffects } from "@/components/ProfileDecoration";
import { ITEM_DECORATION, itemsOfKind, resolveDecoration, resolveEffect } from "@/lib/profile-items";
import { SaveBar, WithPreview } from "@/components/settings/controls";
import { Input } from "@/components/ui/input";
import { T, useI18n } from "@/i18n/react";
import { displayName } from "@/lib/format";
import { useUi } from "@/lib/ui";
import { cn } from "@/lib/utils";

const NICKNAME_MAX = 32;

/**
 * The server being edited. A server picked here counts until another is
 * opened from its menu; one you left falls back to the first.
 */
function usePickedServer(servers: Server[], target: string | null) {
  const [picked, setPicked] = useState<{ id: string; target: string | null } | null>(null);
  const has = (id: string | null): id is string => !!id && servers.some((s) => s.id === id);
  const first = servers[0]?.id ?? "";
  const ours = picked?.target === target ? picked : null;
  const serverId = ours ? (has(ours.id) ? ours.id : first) : has(target) ? target : first;
  return [serverId, (id: string) => setPicked({ id, target })] as const;
}

/** What's being changed on the server profile picked; unset fields are as saved. */
type Edits = { nickname?: string; effect?: string; decoration?: string };

/**
 * A different profile in each server: pick a server, give yourself a
 * nickname there, an effect and a decoration (the fuwa ones and that
 * server's), and see the card people in that server will open.
 */
export function ServerProfiles({ instanceKey }: { instanceKey: string }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const target = useUi((u) => u.settingsTarget);
  const servers = inst?.servers ?? [];
  const [serverId, setPicked] = usePickedServer(servers, target);
  const me = inst?.me;
  const member = useFuwa((s) => s.instances[instanceKey]?.members[serverId]?.find((m) => m.user?.id === me?.id));
  const profile = useFuwa((s) => (me ? s.instances[instanceKey]?.profiles[me.id] : undefined));
  const [edits, setEdits] = useState<Edits>({});
  const save = useAction(updateServerProfile);
  // Server profiles keep an effect and a decoration only on instances with profile items.
  const itemsHere = useFuwa((s) => instanceHas(s.instances[instanceKey]?.node?.versions, "profile-items"));
  const effectsOn = useEffectsOn(instanceKey) && itemsHere;
  const decorationsOn = useDecorationsOn(instanceKey) && itemsHere;
  const items = useFuwa((s) => s.instances[instanceKey]?.serverProfileItems[serverId]);
  const offered = useOfferedEffects(items);

  if (!me) return null;
  if (!servers.length) return <NoServers />;

  const saved = { nickname: member?.nickname ?? "", effect: member?.effect ?? "", decoration: member?.decorationId ?? "" };
  const value = edits.nickname ?? saved.nickname;
  const effect = edits.effect ?? saved.effect;
  const decoration = edits.decoration ?? saved.decoration;
  const patch: ServerProfilePatch = {};
  if (edits.nickname !== undefined && edits.nickname.trim() !== saved.nickname) patch.nickname = edits.nickname.trim();
  if (edits.effect !== undefined && edits.effect !== saved.effect) patch.effect = edits.effect;
  if (edits.decoration !== undefined && edits.decoration !== saved.decoration) patch.decorationId = edits.decoration;
  const changed = Object.keys(patch).length;
  const server = servers.find((s) => s.id === serverId);
  const decorations = itemsOfKind(items, ITEM_DECORATION);
  const set = (next: Edits) => {
    setEdits((e) => ({ ...e, ...next }));
    save.setError(null);
  };
  const pickedEffect = resolveEffect(effect, items);
  const pickedDecoration = resolveDecoration(decoration, items);

  function pick(id: string) {
    if (id === serverId) return;
    setPicked(id);
    setEdits({});
    save.setError(null);
  }

  async function submit(e?: FormEvent) {
    e?.preventDefault();
    if (!changed) return;
    if (await save.go(instanceKey, serverId, patch)) setEdits({});
  }

  const preview = (
    <ProfileCard
      editing
      me
      instanceKey={instanceKey}
      user={me}
      profile={profile}
      member={member ? { ...member, nickname: value.trim(), effect, decorationId: decoration } : undefined}
    />
  );

  return (
    <form onSubmit={submit}>
      <WithPreview preview={preview}>
        <div className="flex flex-col">
          <Row id="server" label={t("accountsettings.serverProfiles.server")}>
            <div role="radiogroup" aria-label={t("accountsettings.serverProfiles.server")} className="flex flex-wrap gap-2">
              {servers.map((s) => (
                <ServerChoice key={s.id} server={s} active={s.id === serverId} locked={changed > 0} onPick={() => pick(s.id)} />
              ))}
            </div>
          </Row>
          <Row
            id="nickname"
            label={t("settings.nav.nickname")}
            htmlFor="server-nickname"
            hint={
              <T
                k="accountsettings.serverProfiles.nicknameHint"
                values={{
                  server: <SwapText className="font-bold text-foreground">{server?.name ?? t("accountsettings.serverProfiles.thisServer")}</SwapText>,
                  name: displayName(me),
                }}
              />
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
                  onChange={(e) => set({ nickname: e.target.value })}
                  className="h-11 max-w-sm rounded-xl"
                />
              </motion.div>
            </AnimatePresence>
          </Row>
          {effectsOn && member && (
            <Row
              id="server-effect"
              label={t("settings.nav.profileEffect")}
              hint={pickedEffect ? <EffectAbout effect={pickedEffect} /> : t("accountsettings.serverProfiles.effectHint")}
            >
              <EffectPicker
                value={effect}
                userId={me.id}
                accent={profile?.accentColor ?? -1}
                offered={offered}
                noneLabel={t("accountsettings.serverProfiles.useMine")}
                pickedId="server-effect-picked"
                onChange={(next) => set({ effect: next })}
              />
            </Row>
          )}
          <AnimatePresence initial={false}>
            {decorationsOn && member && (decorations.length > 0 || decoration) && (
              <motion.div key={`decoration-${serverId}`} {...SLIDE_IN} transition={SPRING}>
                <Row
                  id="server-decoration"
                  label={t("accountsettings.decorations.label")}
                  hint={pickedDecoration?.description || t("accountsettings.serverProfiles.decorationHint")}
                >
                  <DecorationPicker
                    value={decoration}
                    user={me}
                    items={decorations}
                    noneLabel={t("accountsettings.serverProfiles.useMine")}
                    pickedId="server-decoration-picked"
                    onChange={(next) => set({ decoration: next })}
                  />
                </Row>
              </motion.div>
            )}
          </AnimatePresence>
        </div>
        <SaveBar
          count={changed}
          saving={save.pending}
          error={save.error}
          onSave={() => void submit()}
          onDiscard={() => {
            setEdits({});
            save.setError(null);
          }}
        />
      </WithPreview>
    </form>
  );
}

/** Nothing to pick: you're in no servers on this instance. */
function NoServers() {
  const { t } = useI18n();
  return (
    <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={SPRING} className="flex flex-col items-center gap-2 rounded-3xl border border-dashed px-6 py-12 text-center">
      <ServerGlyph className="size-8 text-muted-foreground" />
      <p className="font-extrabold">{t("accountsettings.shared.noServers")}</p>
      <p className="max-w-sm text-sm text-muted-foreground">{t("accountsettings.serverProfiles.noServersHint")}</p>
    </motion.div>
  );
}

/** One server to pick; the others wait while a nickname is unsaved (`locked`). */
function ServerChoice({ server, active, locked, onPick }: { server: Server; active: boolean; locked: boolean; onPick: () => void }) {
  const { t } = useI18n();
  return (
    <motion.button
      type="button"
      role="radio"
      aria-checked={active}
      disabled={locked && !active}
      title={locked && !active ? t("accountsettings.serverProfiles.saveFirst") : server.name}
      whileHover={{ y: -2 }}
      whileTap={{ scale: 0.95 }}
      onClick={onPick}
      className={cn(
        "relative flex max-w-full items-center gap-2 rounded-2xl border py-1.5 pr-3 pl-1.5 text-sm font-bold transition-colors disabled:opacity-50",
        active ? "border-primary/60" : "text-muted-foreground hover:border-primary/30 hover:text-foreground",
      )}
    >
      {active && <motion.span layoutId="server-profile-pick" transition={SPRING} className="absolute inset-0 rounded-2xl bg-primary/10 ring-2 ring-primary/50" />}
      <ServerIcon server={server} className="relative size-7 rounded-lg text-[0.65rem]" />
      <span className="relative truncate">{server.name}</span>
    </motion.button>
  );
}
