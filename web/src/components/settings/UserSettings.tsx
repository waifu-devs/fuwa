import { useParams } from "@tanstack/react-router";
import {
  AccessibilityIcon,
  BellIcon,
  BellRingIcon,
  BotIcon,
  CodeXmlIcon,
  HeartHandshakeIcon,
  DatabaseIcon,
  Flower2Icon,
  IdCardIcon,
  KeyboardIcon,
  KeyRoundIcon,
  LanguagesIcon,
  LogOutIcon,
  MessageSquareTextIcon,
  MonitorSmartphoneIcon,
  PaletteIcon,
  ShieldCheckIcon,
  TvMinimalPlayIcon,
  UserRoundIcon,
  AudioLinesIcon,
  ImageIcon,
  WandSparklesIcon,
} from "lucide-react";
import { useEffect } from "react";
import { useFuwa } from "@/fuwa/store";
import { Accessibility } from "@/components/settings/app/Accessibility";
import { Advanced } from "@/components/settings/app/Advanced";
import { Appearance } from "@/components/settings/app/Appearance";
import { Backgrounds } from "@/components/settings/app/Backgrounds";
import { Themes } from "@/components/settings/app/Themes";
import { Chat } from "@/components/settings/app/Chat";
import { Language } from "@/components/settings/app/Language";
import { keybindSettings, Keybinds } from "@/components/settings/app/Keybinds";
import { Notifications } from "@/components/settings/app/Notifications";
import { Streamer } from "@/components/settings/app/Streamer";
import { Voice, voiceSettings } from "@/components/settings/app/Voice";
import { hasPassword, LinkedSignIn, Password, Session } from "@/components/settings/Account";
import { Agents } from "@/components/settings/account/Agents";
import { Devices } from "@/components/settings/account/Devices";
import { Privacy } from "@/components/settings/account/Privacy";
import { FriendPrivacy } from "@/components/settings/account/FriendPrivacy";
import { Profile } from "@/components/settings/account/Profile";
import { Security } from "@/components/settings/account/Security";
import { ServerNotifications } from "@/components/settings/account/ServerNotifications";
import { ServerProfiles } from "@/components/settings/account/ServerProfiles";
import { SettingsScreen, type SettingsGroup, type SettingsSection } from "@/components/settings/SettingsScreen";
import { type I18n, useI18n } from "@/i18n/react";
import { issuerName } from "@/lib/linked";
import { closeSettings, setSettingsSection, useUi } from "@/lib/ui";

/** The App settings sections, in the app's language (search keywords stay English, beside the words shown). */
function appSections(t: I18n["t"]): SettingsSection[] {
  return [
    {
      id: "appearance",
      label: t("settings.nav.appearance"),
      icon: PaletteIcon,
      description: t("settings.nav.appearanceAbout"),
      keywords: "look theme dark light",
      settings: [
        { id: "theme", label: t("appsettings.appearance.theme"), keywords: "dark light system colors" },
        { id: "density", label: t("appsettings.appearance.density"), keywords: "spacing compact spacious" },
        { id: "message-display", label: t("appsettings.appearance.display"), keywords: "cozy compact" },
        { id: "chat-font-size", label: t("settings.nav.chatTextSize"), keywords: "font scaling" },
        { id: "zoom", label: t("appsettings.appearance.zoom"), keywords: "scale size" },
      ],
    },
    {
      id: "themes",
      label: t("settings.nav.themes"),
      icon: WandSparklesIcon,
      description: t("settings.nav.themesAbout"),
      keywords: "custom theme colors import export file editor",
    },
    {
      id: "backdrop",
      label: t("settings.nav.backdrop"),
      icon: ImageIcon,
      description: t("settings.nav.backdropAbout"),
      keywords: "wallpaper background picture image effect shader aurora petals stars waves grain texture",
      settings: [{ id: "backdrop", label: t("appsettings.backgrounds.title"), keywords: "wallpaper picture shader texture" }],
    },
    {
      id: "accessibility",
      label: t("settings.nav.accessibility"),
      icon: AccessibilityIcon,
      description: t("settings.nav.accessibilityAbout"),
      keywords: "a11y",
      settings: [
        { id: "reduce-motion", label: t("settings.nav.reduceMotion"), keywords: "animation" },
        { id: "saturation", label: t("appsettings.accessibility.saturation"), keywords: "color grey" },
        { id: "role-colors", label: t("appsettings.accessibility.roleColors"), keywords: "names colour tint dot" },
        { id: "others-effects", label: t("appsettings.accessibility.effects"), keywords: "animation sparkles others cards" },
        { id: "underline-links", label: t("settings.nav.underlineLinks") },
      ],
    },
    {
      id: "chat",
      label: t("settings.nav.chat"),
      icon: MessageSquareTextIcon,
      description: t("settings.nav.chatAbout"),
      keywords: "messages",
      settings: [
        { id: "clock", label: t("appsettings.chat.clock"), keywords: "12 24 hour clock" },
        { id: "send-with", label: t("appsettings.chat.sendWith"), keywords: "enter newline" },
      ],
    },
    {
      id: "language",
      label: t("settings.language.title"),
      icon: LanguagesIcon,
      description: t("settings.nav.languageAbout"),
      keywords: "locale translation idioma español",
      settings: [{ id: "language", label: t("settings.language.title"), keywords: "locale translation" }],
    },
    {
      id: "notifications",
      label: t("settings.nav.notifications"),
      icon: BellIcon,
      description: t("settings.nav.notificationsAbout"),
      keywords: "alerts",
      settings: [
        { id: "desktop-notifications", label: t("appsettings.notifications.desktop") },
        { id: "notify-for", label: t("appsettings.notifications.notifyFor"), keywords: "mentions every message" },
        { id: "unread-badge", label: t("appsettings.notifications.unreadBadge"), keywords: "badge title favicon" },
        { id: "sounds", label: t("appsettings.notifications.sounds"), keywords: "volume audio" },
      ],
    },
    {
      id: "voice",
      label: t("settings.nav.voice"),
      icon: AudioLinesIcon,
      description: t("settings.nav.voiceAbout"),
      keywords: "call microphone mic speakers headset audio camera video webcam",
      settings: voiceSettings(t),
    },
    {
      id: "keybinds",
      label: t("settings.nav.keybinds"),
      icon: KeyboardIcon,
      description: t("settings.nav.keybindsAbout"),
      keywords: "keyboard shortcuts hotkeys",
      settings: keybindSettings(t),
    },
    {
      id: "streamer",
      label: t("appsettings.streamer.title"),
      icon: TvMinimalPlayIcon,
      description: t("settings.nav.streamerAbout"),
      keywords: "privacy obs stream hide",
      settings: [
        { id: "streamer-hide-personal", label: t("appsettings.streamer.hidePersonal"), keywords: "address username" },
        { id: "streamer-sounds", label: t("settings.nav.streamerSounds") },
        { id: "streamer-notifications", label: t("settings.nav.streamerNotifications") },
      ],
    },
    {
      id: "advanced",
      label: t("settings.nav.advanced"),
      icon: CodeXmlIcon,
      description: t("settings.nav.advancedAbout"),
      settings: [
        { id: "share-reports", label: t("appsettings.advanced.reports"), keywords: "anonymous reports telemetry crash errors performance privacy" },
        { id: "developer-mode", label: t("appsettings.advanced.developer"), keywords: "copy id" },
      ],
    },
  ];
}

const ACCOUNT = new Set(["profile", "server-profiles", "devices", "security", "password", "server-notifications", "agents", "privacy", "session"]);

/**
 * Settings, opened from anywhere (the user panel, a shortcut, a link): App
 * settings belong to this device and every instance on it; the account
 * group belongs to the instance on screen.
 */
export function UserSettings() {
  const { t } = useI18n();
  const open = useUi((u) => u.settings);
  const { instance: key } = useParams({ strict: false }) as { instance?: string };
  // Always mounted (settings open from anywhere), so it reads only what it shows.
  const me = useFuwa((s) => (key ? s.instances[key]?.me : undefined));
  const node = useFuwa((s) => (key ? s.instances[key]?.node : undefined));
  const where = node?.name ?? t("settings.nav.thisInstance");

  // Signing out leaves nothing on the account pages to show.
  useEffect(() => {
    if (open && ACCOUNT.has(open) && !me) closeSettings();
  }, [open, me]);

  const groups: SettingsGroup[] = [{ label: t("settings.nav.app"), sections: appSections(t) }];
  if (key && me) {
    groups.push({
      label: t("settings.nav.account", { instance: where }),
      sections: [
        {
          id: "profile",
          label: t("settings.nav.profile"),
          icon: UserRoundIcon,
          description: t("settings.nav.profileAbout", { instance: where }),
          keywords: "name avatar picture",
          settings: [
            { id: "display-name", label: t("settings.nav.displayName") },
            { id: "pronouns", label: t("settings.nav.pronouns") },
            { id: "avatar", label: t("settings.nav.avatar"), keywords: "picture photo upload image gif" },
            { id: "banner", label: t("settings.nav.banner"), keywords: "header picture upload image" },
            { id: "profile-color", label: t("settings.nav.profileColor"), keywords: "accent" },
            { id: "profile-effect", label: t("settings.nav.profileEffect"), keywords: "sparkles petals stars hearts snow confetti animation decoration" },
            { id: "status", label: t("settings.nav.status"), keywords: "away busy" },
            { id: "about-me", label: t("settings.nav.aboutMe"), keywords: "bio description" },
          ],
        },
        {
          id: "server-profiles",
          label: t("settings.nav.serverProfiles"),
          icon: IdCardIcon,
          description: t("settings.nav.serverProfilesAbout"),
          keywords: "per server identity",
          settings: [{ id: "nickname", label: t("settings.nav.nickname"), keywords: "server name" }],
        },
        {
          id: "devices",
          label: t("settings.nav.devices"),
          icon: MonitorSmartphoneIcon,
          description: t("settings.nav.devicesAbout", { instance: where }),
          keywords: "sessions sign out log out phone browser",
        },
        ...(hasPassword(me)
          ? [
              {
                id: "security",
                label: t("settings.nav.twoStep"),
                icon: ShieldCheckIcon,
                description: t("settings.nav.twoStepAbout"),
                keywords: "2fa mfa totp authenticator backup codes security",
                settings: [
                  { id: "two-step", label: t("settings.nav.twoStep"), keywords: "2fa authenticator" },
                  { id: "backup-codes", label: t("settings.nav.backupCodes"), keywords: "recovery" },
                ],
              },
              {
                id: "password",
                label: t("settings.nav.password"),
                icon: KeyRoundIcon,
                description: t("settings.nav.passwordAbout", { instance: where }),
                keywords: "security change",
              },
            ]
          : [
              {
                id: "linked",
                label: t("settings.nav.linked"),
                icon: Flower2Icon,
                description: t("settings.nav.linkedAbout", { instance: where, issuer: issuerName(node?.auth?.linkedIssuer) }),
                keywords: "waifu.dev linked password 2fa security",
              },
            ]),
        {
          id: "server-notifications",
          label: t("settings.nav.serverNotifications"),
          icon: BellRingIcon,
          description: t("settings.nav.serverNotificationsAbout"),
          keywords: "mute mentions everyone here alerts",
        },
        {
          id: "agents",
          label: t("settings.nav.agents"),
          icon: BotIcon,
          description: t("settings.nav.agentsAbout"),
          keywords: "bot bots token api key automation integration developer",
        },
        {
          id: "friends",
          label: t("settings.nav.friends"),
          icon: HeartHandshakeIcon,
          description: t("settings.nav.friendsAbout"),
          keywords: "friend requests direct messages dm online status mutual block",
          settings: [
            { id: "friend-requests", label: t("settings.nav.friendRequests"), keywords: "requests add" },
            { id: "direct-messages", label: t("settings.nav.directMessages"), keywords: "dm messages" },
            { id: "friends-see", label: t("settings.nav.friendsSee"), keywords: "online mutual" },
          ],
        },
        {
          id: "privacy",
          label: t("settings.nav.privacy"),
          icon: DatabaseIcon,
          description: t("settings.nav.privacyAbout", { instance: where }),
          keywords: "export download delete account gdpr activity rich presence game playing status",
          settings: [
            { id: "activity-sharing", label: t("settings.nav.activity"), keywords: "rich presence activity game playing listening discord share" },
            { id: "export", label: t("settings.nav.export"), keywords: "export json" },
            { id: "delete-account", label: t("settings.nav.deleteAccount"), keywords: "remove close" },
          ],
        },
      ],
    });
    groups.push({ sections: [{ id: "session", label: t("settings.nav.signOut"), icon: LogOutIcon, danger: true, keywords: "log out remove" }] });
  }

  const known = groups.some((g) => g.sections.some((s) => s.id === open));
  const section = open && known ? open : "appearance";

  return (
    <SettingsScreen
      open={open !== null}
      onOpenChange={(next) => !next && closeSettings()}
      title={t("settings.screen.title")}
      subtitle={me ? t("settings.nav.signedIn", { instance: where }) : t("settings.nav.thisDevice")}
      groups={groups}
      section={section}
      onSectionChange={setSettingsSection}
    >
      {section === "appearance" && <Appearance instanceKey={key} />}
      {section === "themes" && <Themes instanceKey={key} />}
      {section === "backdrop" && <Backgrounds instanceKey={key} />}
      {section === "accessibility" && <Accessibility />}
      {section === "chat" && <Chat />}
      {section === "language" && <Language />}
      {section === "notifications" && <Notifications />}
      {section === "voice" && <Voice />}
      {section === "keybinds" && <Keybinds />}
      {section === "streamer" && <Streamer instanceKey={key} />}
      {section === "advanced" && <Advanced />}
      {key && section === "profile" && <Profile instanceKey={key} />}
      {key && section === "server-profiles" && <ServerProfiles instanceKey={key} />}
      {key && section === "devices" && <Devices instanceKey={key} />}
      {key && section === "security" && <Security instanceKey={key} />}
      {key && section === "password" && <Password instanceKey={key} />}
      {key && section === "linked" && <LinkedSignIn instanceKey={key} />}
      {key && section === "server-notifications" && <ServerNotifications instanceKey={key} />}
      {key && section === "agents" && <Agents instanceKey={key} />}
      {key && section === "friends" && <FriendPrivacy instanceKey={key} />}
      {key && section === "privacy" && <Privacy instanceKey={key} />}
      {key && section === "session" && <Session instanceKey={key} />}
    </SettingsScreen>
  );
}
