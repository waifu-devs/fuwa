import { useMemo, useState } from "react";
import type { Member, ProfileItem, User } from "@/gen/fuwa/v1/types_pb";
import { FirstFrame } from "@/components/EmojiImage";
import { useFuwa } from "@/fuwa/store";
import { ITEM_EFFECT, itemEffect, itemsOfKind, wornDecoration, wornEffect, type ItemLists } from "@/lib/profile-items";
import type { ProfileEffectSpec } from "@/lib/effects/profile";
import { reduceMotion, usePrefs } from "@/lib/prefs";
import { shownPicture } from "@/lib/shown";
import { cn } from "@/lib/utils";

const NONE: ProfileItem[] = [];

/** The instance's profile items and, seen in a server, that server's (docs/profile-items.md). */
export function useItemLists(instanceKey: string | undefined, serverId: string | undefined): ItemLists<ProfileItem> {
  const instance = useFuwa((s) => (instanceKey ? s.instances[instanceKey]?.profileItems : undefined)) ?? NONE;
  const server = useFuwa((s) => (instanceKey && serverId ? s.instances[instanceKey]?.serverProfileItems[serverId] : undefined));
  return { instance, server };
}

/** Whether the instance lets decorations show at all (`Node.profile_decorations`; off on instances from before them). */
export const useDecorationsOn = (instanceKey: string | undefined) => useFuwa((s) => !!(instanceKey && s.instances[instanceKey]?.node?.profileDecorations));

/** Whether the instance lets profile effects play (`Node.profile_effects`). */
export const useEffectsOn = (instanceKey: string | undefined) => useFuwa((s) => !!(instanceKey && s.instances[instanceKey]?.node?.profileEffects));

/** The decoration someone shows where they're seen: their server profile's in a server, else their own. */
export function useDecoration(instanceKey: string | undefined, user: Pick<User, "decorationId"> | undefined, member?: Pick<Member, "serverId" | "decorationId">) {
  const on = useDecorationsOn(instanceKey);
  const lists = useItemLists(instanceKey, member?.serverId);
  return on ? wornDecoration(user, member, lists) : undefined;
}

/** The effect someone shows where they're seen, from their profile's pick and their server profile's. */
export function useWornEffect(instanceKey: string | undefined, own: string | undefined, member?: Pick<Member, "serverId" | "effect">) {
  const lists = useItemLists(instanceKey, member?.serverId);
  return wornEffect(own, member, lists);
}

/**
 * A decoration's picture, centred over the avatar it sits in (the parent,
 * `relative` and the avatar's size) at 1.2 times its size. It never takes
 * clicks or space, readers skip it, and a moving one holds its first frame
 * under Reduce motion. Only pictures on a fuwa instance load (`shownPicture`).
 */
export function DecorationImage({ item, className }: { item: Pick<ProfileItem, "pictureUrl" | "animated"> | undefined; className?: string }) {
  const src = shownPicture(item?.pictureUrl);
  if (!item || !src) return null;
  return <Picture key={src} src={src} animated={item.animated} className={className} />;
}

function Picture({ src, animated, className }: { src: string; animated: boolean; className?: string }) {
  const calm = usePrefs(reduceMotion);
  const [broken, setBroken] = useState(false);
  if (broken) return null;
  const shared = {
    "aria-hidden": true,
    draggable: false,
    className: cn("pointer-events-none absolute -inset-[10%] z-[1] size-[120%] max-w-none object-contain select-none", className),
  } as const;
  if (calm && animated) return <FirstFrame src={src} onBroken={setBroken} {...shared} />;
  return <img src={src} alt="" onError={() => setBroken(true)} {...shared} />;
}

/** Someone's decoration over their avatar, where they're seen (a server, through `member`, or anywhere on the instance). */
export function AvatarDecoration({
  instanceKey,
  user,
  member,
  className,
}: {
  instanceKey: string | undefined;
  user: Pick<User, "decorationId"> | undefined;
  member?: Pick<Member, "serverId" | "decorationId">;
  className?: string;
}) {
  const item = useDecoration(instanceKey, user, member);
  return <DecorationImage item={item} className={className} />;
}

/** The effects an instance or a server offers, as pickers take them: each one that can play, oldest first. */
export function useOfferedEffects(items: readonly ProfileItem[] | undefined): { id: string; spec: ProfileEffectSpec }[] {
  return useMemo(
    () =>
      itemsOfKind(items, ITEM_EFFECT).flatMap((item) => {
        const spec = itemEffect(item);
        return spec ? [{ id: item.id, spec }] : [];
      }),
    [items],
  );
}
