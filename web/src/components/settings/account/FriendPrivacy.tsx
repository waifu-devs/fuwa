import { create } from "@bufbuild/protobuf";
import { BanIcon, EarthIcon, HeartIcon, MessageCircleOffIcon, ServerIcon, UsersIcon } from "lucide-react";
import { DirectMessagesFrom, FriendRequestsFrom, FriendSettingsSchema, type FriendSettings } from "@/gen/fuwa/v1/friend_pb";
import { run } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { saveFriendSettings } from "@/fuwa/friends";
import { useFuwa } from "@/fuwa/store";
import { Choice, Setting, Toggle, type ChoiceOption } from "@/components/settings/controls";
import { type I18n, useI18n } from "@/i18n/react";
import { toast } from "@/lib/ui";

const requests = (t: I18n["t"]): ChoiceOption<FriendRequestsFrom>[] => [
  { value: FriendRequestsFrom.UNSPECIFIED, label: t("accountsettings.friends.everyone"), hint: t("accountsettings.friends.everyoneHint"), icon: <EarthIcon className="size-4" /> },
  {
    value: FriendRequestsFrom.SHARED_SERVERS,
    label: t("accountsettings.friends.serverFriends"),
    hint: t("accountsettings.friends.serverFriendsHint"),
    icon: <ServerIcon className="size-4" />,
  },
  { value: FriendRequestsFrom.NOBODY, label: t("accountsettings.friends.nobody"), hint: t("accountsettings.friends.nobodyHint"), icon: <BanIcon className="size-4" /> },
];

const messages = (t: I18n["t"]): ChoiceOption<DirectMessagesFrom>[] => [
  {
    value: DirectMessagesFrom.UNSPECIFIED,
    label: t("accountsettings.friends.everyone"),
    hint: t("accountsettings.friends.messagesEveryoneHint"),
    icon: <UsersIcon className="size-4" />,
  },
  { value: DirectMessagesFrom.FRIENDS, label: t("accountsettings.friends.friendsOnly"), hint: t("accountsettings.friends.friendsOnlyHint"), icon: <HeartIcon className="size-4" /> },
  {
    value: DirectMessagesFrom.NOBODY,
    label: t("accountsettings.friends.nobodyNew"),
    hint: t("accountsettings.friends.nobodyNewHint"),
    icon: <MessageCircleOffIcon className="size-4" />,
  },
];

/**
 * Who may ask you to be friends or start a conversation with you, and what
 * your friends see. Kept on the instance, so it holds on every device;
 * each change saves at once. Blocking someone (from their profile or your
 * friends list) stops them whatever these say.
 */
export function FriendPrivacy({ instanceKey }: { instanceKey: string }) {
  const { t } = useI18n();
  const settings = useFuwa((s) => s.instances[instanceKey]?.friends.settings);
  const status = useFuwa((s) => s.instances[instanceKey]?.friends.status ?? "off");
  if (status === "unsupported") return <p className="text-sm text-muted-foreground">{t("accountsettings.friends.unsupported")}</p>;
  const current = settings ?? create(FriendSettingsSchema);
  const save = (patch: Partial<FriendSettings>) =>
    run(saveFriendSettings(instanceKey, create(FriendSettingsSchema, { ...current, ...patch }))).catch((err: FuwaError) => toast(err.message));

  return (
    <div>
      <Setting
        id="friend-requests"
        title={t("settings.nav.friendRequests")}
        hint={t("accountsettings.friends.requestsHint")}
        changed={current.requestsFrom !== FriendRequestsFrom.UNSPECIFIED}
        defaultLabel={t("accountsettings.friends.everyone")}
        onReset={() => void save({ requestsFrom: FriendRequestsFrom.UNSPECIFIED })}
      >
        <Choice value={current.requestsFrom} options={requests(t)} onChange={(requestsFrom) => void save({ requestsFrom })} />
      </Setting>
      <Setting
        id="direct-messages"
        title={t("settings.nav.directMessages")}
        hint={t("accountsettings.friends.messagesHint")}
        changed={current.directMessagesFrom !== DirectMessagesFrom.UNSPECIFIED}
        defaultLabel={t("accountsettings.friends.everyone")}
        onReset={() => void save({ directMessagesFrom: DirectMessagesFrom.UNSPECIFIED })}
        delay={0.04}
      >
        <Choice value={current.directMessagesFrom} options={messages(t)} onChange={(directMessagesFrom) => void save({ directMessagesFrom })} />
      </Setting>
      <Setting
        id="friends-see"
        title={t("settings.nav.friendsSee")}
        changed={current.hideOnline || current.hideMutualFriends}
        defaultLabel={t("accountsettings.friends.bothShown")}
        onReset={() => void save({ hideOnline: false, hideMutualFriends: false })}
        delay={0.08}
      >
        <div className="flex flex-col gap-4">
          <Toggle
            label={t("accountsettings.friends.showOnline")}
            hint={t("accountsettings.friends.showOnlineHint")}
            checked={!current.hideOnline}
            onChange={(on) => void save({ hideOnline: !on })}
          />
          <Toggle
            label={t("accountsettings.friends.showMutual")}
            hint={t("accountsettings.friends.showMutualHint")}
            checked={!current.hideMutualFriends}
            onChange={(on) => void save({ hideMutualFriends: !on })}
          />
        </div>
      </Setting>
    </div>
  );
}
