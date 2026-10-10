import { Code } from "@connectrpc/connect";
import { Effect, Schedule, Stream } from "effect";
import type { FriendSettings, WatchFriendsResponse } from "@/gen/fuwa/v1/friend_pb";
import { applyFriendEvent, FRIEND, INCOMING, OUTGOING, sortFriends, stateWith, type FriendsState } from "@/lib/friends";
import { onFriendNews } from "@/lib/notify";
import { reportUsage } from "@/lib/reports";
import type { Api } from "./client";
import { call, FuwaError, toFuwaError } from "./errors";
import { i18n } from "@/i18n/i18n";
import { engine } from "./sync";
import { store, updateInstance } from "./store";

/**
 * Friends on one instance: the list kept in step over `WatchFriends` (which
 * is also what shows you online to your friends), and what people do with it.
 */

const updateFriends = (key: string, fn: (f: FriendsState) => FriendsState) =>
  updateInstance(key, (i) => {
    const friends = fn(i.friends);
    return friends === i.friends ? i : { ...i, friends };
  });

const backoff = Schedule.exponential("500 millis", 2).pipe(Schedule.union(Schedule.spaced("20 seconds")), Schedule.jittered);
/** Heartbeats come every 25 seconds; this long without one means the stream is gone. */
const SILENCE = "70 seconds";
const ENDED = new FuwaError({ code: Code.Unavailable, message: "the server closed the connection" });

/**
 * One response from the friends feed, on WatchFriends or a live connection:
 * at `ready` (listening again) the whole list is read, so whatever happened
 * while away is in; after that, each change.
 */
export const onFriendsResponse = (key: string, api: Api, res: WatchFriendsResponse) =>
  res.ready
    ? readFriends(api).pipe(Effect.tap((read) => Effect.sync(() => setFriends(key, read))), Effect.asVoid)
    : Effect.sync(() => {
        const event = res.event;
        if (!event) return;
        const before = store.get().instances[key]?.friends.list ?? [];
        if (event.payload.case === "settings") {
          const settings = event.payload.value;
          updateFriends(key, (f) => ({ ...f, settings }));
          return;
        }
        updateFriends(key, (f) => {
          const list = applyFriendEvent(f.list, event);
          return list === f.list ? f : { ...f, list };
        });
        if (event.payload.case === "changed") {
          const friend = event.payload.value;
          const was = stateWith(before, friend.user?.id);
          if (friend.state === INCOMING && was !== INCOMING) onFriendNews(key, friend.user, "asked");
          if (friend.state === FRIEND && was === OUTGOING) onFriendNews(key, friend.user, "accepted");
        }
      });

/** Your friends and settings, read whole (at a feed's `ready`). */
export const readFriends = (api: Api) =>
  Effect.all([
    call((signal) => api.friends.listFriends({}, { signal })),
    call((signal) => api.friends.getFriendSettings({}, { signal })),
  ]).pipe(Effect.map(([list, settings]) => ({ list: sortFriends(list.friends), settings: settings.settings ?? null })));

/** Puts what `readFriends` read in place of what was known. */
export const setFriends = (key: string, read: { list: FriendsState["list"]; settings: FriendSettings | null }) =>
  updateFriends(key, () => ({ status: "ready", ...read }));

/** Friends are loading, until the friends feed's `ready` lists them. */
export const friendsLoading = (key: string) =>
  updateFriends(key, (f) => ({ ...f, status: f.status === "ready" ? "ready" : "loading" }));

/**
 * Lists your friends, then follows changes for as long as the instance is
 * synced, listing again after each reconnect so nothing is missed. An
 * instance from before friends simply has none. On a live connection the
 * same responses come over it instead (`onFriendsResponse`).
 */
export const followFriends = (key: string, api: Api) =>
  Effect.gen(function* () {
    friendsLoading(key);
    const watch = Stream.suspend(() => {
      const controller = new AbortController();
      return Stream.fromAsyncIterable<WatchFriendsResponse, FuwaError>(
        api.friends.watchFriends({}, { signal: controller.signal }),
        toFuwaError,
      ).pipe(
        Stream.concat(Stream.fail(ENDED)),
        Stream.ensuring(Effect.sync(() => controller.abort())),
      );
    }).pipe(
      Stream.timeoutFail(() => new FuwaError({ code: Code.Unavailable, message: i18n().t("system.connection.lost") }), SILENCE),
      Stream.mapEffect((res) => onFriendsResponse(key, api, res)),
      Stream.retry(backoff.pipe(Schedule.whileInput((e: FuwaError) => e.retryable))),
    );
    yield* Stream.runDrain(watch);
  }).pipe(
    Effect.catchAll((err) =>
      Effect.sync(() => {
        if (err.code === Code.Unimplemented) updateFriends(key, (f) => ({ ...f, status: "unsupported" }));
      }),
    ),
  );

const api = (key: string) => engine(key).api;

/** Asks someone to be friends, by id or username. */
export const sendFriendRequest = (key: string, who: { userId?: string; username?: string }) =>
  call((signal) => api(key).friends.sendFriendRequest({ userId: who.userId ?? "", username: who.username ?? "" }, { signal })).pipe(
    Effect.tap(({ friend }) =>
      Effect.sync(() => {
        reportUsage("friends.request");
        if (friend) updateFriends(key, (f) => ({ ...f, list: applyFriendEvent(f.list, { payload: { case: "changed", value: friend } } as never) }));
      }),
    ),
    Effect.map(({ friend }) => friend),
  );

export const acceptFriend = (key: string, userId: string) =>
  call((signal) => api(key).friends.acceptFriendRequest({ userId }, { signal })).pipe(
    Effect.tap(({ friend }) =>
      Effect.sync(() => {
        reportUsage("friends.accept");
        if (friend) updateFriends(key, (f) => ({ ...f, list: applyFriendEvent(f.list, { payload: { case: "changed", value: friend } } as never) }));
      }),
    ),
  );

const drop = (key: string, userId: string) =>
  updateFriends(key, (f) => ({ ...f, list: applyFriendEvent(f.list, { payload: { case: "removed", value: userId } } as never) }));

/** Unfriends someone, declines their request or cancels yours. */
export const removeFriend = (key: string, userId: string) =>
  call((signal) => api(key).friends.removeFriend({ userId }, { signal })).pipe(Effect.tap(() => Effect.sync(() => drop(key, userId))));

export const blockUser = (key: string, userId: string) =>
  call((signal) => api(key).friends.blockUser({ userId }, { signal })).pipe(
    Effect.tap(({ friend }) =>
      Effect.sync(() => {
        reportUsage("friends.block");
        if (friend) updateFriends(key, (f) => ({ ...f, list: applyFriendEvent(f.list, { payload: { case: "changed", value: friend } } as never) }));
      }),
    ),
  );

export const unblockUser = (key: string, userId: string) =>
  call((signal) => api(key).friends.unblockUser({ userId }, { signal })).pipe(Effect.tap(() => Effect.sync(() => drop(key, userId))));

/** Where you stand with someone, with mutual friends, for their profile card. */
export const getRelationship = (key: string, userId: string) => call((signal) => api(key).friends.getRelationship({ userId }, { signal }));

/** Saves your privacy settings; the screen shows them at once and puts them back if this fails. */
export const saveFriendSettings = (key: string, settings: FriendSettings) => {
  const before = store.get().instances[key]?.friends.settings ?? null;
  updateFriends(key, (f) => ({ ...f, settings }));
  return call((signal) => api(key).friends.updateFriendSettings({ settings }, { signal })).pipe(
    Effect.tapError(() => Effect.sync(() => updateFriends(key, (f) => ({ ...f, settings: before })))),
  );
};
