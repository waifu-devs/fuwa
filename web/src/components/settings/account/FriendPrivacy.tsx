import { create } from "@bufbuild/protobuf";
import { BanIcon, EarthIcon, HeartIcon, MessageCircleOffIcon, ServerIcon, UsersIcon } from "lucide-react";
import { DirectMessagesFrom, FriendRequestsFrom, FriendSettingsSchema, type FriendSettings } from "@/gen/fuwa/v1/friend_pb";
import { run } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { saveFriendSettings } from "@/fuwa/friends";
import { useFuwa } from "@/fuwa/store";
import { Choice, Setting, Toggle, type ChoiceOption } from "@/components/settings/controls";
import { toast } from "@/lib/ui";

const REQUESTS: ChoiceOption<FriendRequestsFrom>[] = [
  { value: FriendRequestsFrom.UNSPECIFIED, label: "Everyone", hint: "Anyone on this instance", icon: <EarthIcon className="size-4" /> },
  { value: FriendRequestsFrom.SHARED_SERVERS, label: "Server friends", hint: "People in a server with you", icon: <ServerIcon className="size-4" /> },
  { value: FriendRequestsFrom.NOBODY, label: "Nobody", hint: "You can still send them", icon: <BanIcon className="size-4" /> },
];

const MESSAGES: ChoiceOption<DirectMessagesFrom>[] = [
  { value: DirectMessagesFrom.UNSPECIFIED, label: "Everyone", hint: "Friends and people in a server with you", icon: <UsersIcon className="size-4" /> },
  { value: DirectMessagesFrom.FRIENDS, label: "Friends only", hint: "Only friends can start one", icon: <HeartIcon className="size-4" /> },
  { value: DirectMessagesFrom.NOBODY, label: "Nobody new", hint: "Conversations you have keep going", icon: <MessageCircleOffIcon className="size-4" /> },
];

/**
 * Who may ask you to be friends or start a conversation with you, and what
 * your friends see. Kept on the instance, so it holds on every device;
 * each change saves at once. Blocking someone (from their profile or your
 * friends list) stops them whatever these say.
 */
export function FriendPrivacy({ instanceKey }: { instanceKey: string }) {
  const settings = useFuwa((s) => s.instances[instanceKey]?.friends.settings);
  const status = useFuwa((s) => s.instances[instanceKey]?.friends.status ?? "off");
  if (status === "unsupported") return <p className="text-sm text-muted-foreground">This instance runs a version of fuwa from before friends.</p>;
  const current = settings ?? create(FriendSettingsSchema);
  const save = (patch: Partial<FriendSettings>) =>
    run(saveFriendSettings(instanceKey, create(FriendSettingsSchema, { ...current, ...patch }))).catch((err: FuwaError) => toast(err.message));

  return (
    <div>
      <Setting
        id="friend-requests"
        title="Who can send you friend requests"
        hint="Requests from anyone else never reach you; they're told you aren't taking them."
        changed={current.requestsFrom !== FriendRequestsFrom.UNSPECIFIED}
        defaultLabel="Everyone"
        onReset={() => void save({ requestsFrom: FriendRequestsFrom.UNSPECIFIED })}
      >
        <Choice value={current.requestsFrom} options={REQUESTS} onChange={(requestsFrom) => void save({ requestsFrom })} />
      </Setting>
      <Setting
        id="direct-messages"
        title="Who can start a conversation with you"
        hint="Direct messages stay end-to-end encrypted whoever sends them."
        changed={current.directMessagesFrom !== DirectMessagesFrom.UNSPECIFIED}
        defaultLabel="Everyone"
        onReset={() => void save({ directMessagesFrom: DirectMessagesFrom.UNSPECIFIED })}
        delay={0.04}
      >
        <Choice value={current.directMessagesFrom} options={MESSAGES} onChange={(directMessagesFrom) => void save({ directMessagesFrom })} />
      </Setting>
      <Setting
        id="friends-see"
        title="What your friends see"
        changed={current.hideOnline || current.hideMutualFriends}
        defaultLabel="Both shown"
        onReset={() => void save({ hideOnline: false, hideMutualFriends: false })}
        delay={0.08}
      >
        <div className="flex flex-col gap-4">
          <Toggle
            label="Show when I'm online"
            hint="Friends see a green dot while you have fuwa open. Nobody else ever does."
            checked={!current.hideOnline}
            onChange={(on) => void save({ hideOnline: !on })}
          />
          <Toggle
            label="Show mutual friends"
            hint="On profiles, only when you, they and the friend you share all allow it."
            checked={!current.hideMutualFriends}
            onChange={(on) => void save({ hideMutualFriends: !on })}
          />
        </div>
      </Setting>
    </div>
  );
}
