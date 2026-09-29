import { useParams } from "@tanstack/react-router";
import {
  AccessibilityIcon,
  BellIcon,
  CodeXmlIcon,
  KeyboardIcon,
  KeyRoundIcon,
  LogOutIcon,
  MessageSquareTextIcon,
  PaletteIcon,
  TvMinimalPlayIcon,
  UserRoundIcon,
} from "lucide-react";
import { useEffect } from "react";
import { useInstance } from "@/fuwa/hooks";
import { Accessibility } from "@/components/settings/app/Accessibility";
import { Advanced } from "@/components/settings/app/Advanced";
import { Appearance } from "@/components/settings/app/Appearance";
import { Chat } from "@/components/settings/app/Chat";
import { KEYBIND_SETTINGS, Keybinds } from "@/components/settings/app/Keybinds";
import { Notifications } from "@/components/settings/app/Notifications";
import { Streamer } from "@/components/settings/app/Streamer";
import { hasPassword, Password, Profile, Session } from "@/components/settings/Account";
import { SettingsScreen, type SettingsGroup, type SettingsSection } from "@/components/settings/SettingsScreen";
import { closeSettings, setSettingsSection, useUi } from "@/lib/ui";

const APP: SettingsSection[] = [
  {
    id: "appearance",
    label: "Appearance",
    icon: PaletteIcon,
    description: "How fuwa looks on this device, for every instance you use here.",
    keywords: "look theme dark light",
    settings: [
      { id: "theme", label: "Theme", keywords: "dark light system colors" },
      { id: "density", label: "Density", keywords: "spacing compact spacious" },
      { id: "message-display", label: "Message display", keywords: "cozy compact" },
      { id: "chat-font-size", label: "Chat text size", keywords: "font scaling" },
      { id: "zoom", label: "Zoom", keywords: "scale size" },
    ],
  },
  {
    id: "accessibility",
    label: "Accessibility",
    icon: AccessibilityIcon,
    description: "Motion, color and links, for every instance on this device.",
    keywords: "a11y",
    settings: [
      { id: "reduce-motion", label: "Reduce motion", keywords: "animation" },
      { id: "saturation", label: "Saturation", keywords: "color grey" },
      { id: "underline-links", label: "Underline links" },
    ],
  },
  {
    id: "chat",
    label: "Chat",
    icon: MessageSquareTextIcon,
    description: "How messages read and send.",
    keywords: "messages",
    settings: [
      { id: "clock", label: "Time format", keywords: "12 24 hour clock" },
      { id: "send-with", label: "Send messages with", keywords: "enter newline" },
    ],
  },
  {
    id: "notifications",
    label: "Notifications",
    icon: BellIcon,
    description: "What reaches you on this device while fuwa is open.",
    keywords: "alerts",
    settings: [
      { id: "desktop-notifications", label: "Desktop notifications" },
      { id: "notify-for", label: "Notify me about", keywords: "mentions every message" },
      { id: "unread-badge", label: "Unread count on the tab", keywords: "badge title favicon" },
      { id: "sounds", label: "Sounds", keywords: "volume audio" },
    ],
  },
  {
    id: "keybinds",
    label: "Keybinds",
    icon: KeyboardIcon,
    description: "Shortcuts for getting around without the mouse.",
    keywords: "keyboard shortcuts hotkeys",
    settings: KEYBIND_SETTINGS,
  },
  {
    id: "streamer",
    label: "Streamer mode",
    icon: TvMinimalPlayIcon,
    description: "Hides what a stream shouldn't show.",
    keywords: "privacy obs stream hide",
    settings: [
      { id: "streamer-hide-personal", label: "Hide personal information", keywords: "address username" },
      { id: "streamer-sounds", label: "Mute sounds while streaming" },
      { id: "streamer-notifications", label: "Mute notifications while streaming" },
    ],
  },
  {
    id: "advanced",
    label: "Advanced",
    icon: CodeXmlIcon,
    description: "Tools for people who build on fuwa.",
    settings: [{ id: "developer-mode", label: "Developer mode", keywords: "copy id" }],
  },
];

const ACCOUNT = new Set(["profile", "password", "session"]);

/**
 * Settings, opened from anywhere (the user panel, a shortcut, a link): App
 * settings belong to this device and every instance on it; the account
 * group belongs to the instance on screen.
 */
export function UserSettings() {
  const open = useUi((u) => u.settings);
  const { instance: key } = useParams({ strict: false }) as { instance?: string };
  const inst = useInstance(key);
  const me = inst?.me;
  const where = inst?.node?.name ?? "this instance";

  // Signing out leaves nothing on the account pages to show.
  useEffect(() => {
    if (open && ACCOUNT.has(open) && !me) closeSettings();
  }, [open, me]);

  const groups: SettingsGroup[] = [{ label: "App settings", sections: APP }];
  if (key && me) {
    groups.push({
      label: `Your account on ${where}`,
      sections: [
        {
          id: "profile",
          label: "Profile",
          icon: UserRoundIcon,
          description: `How people see you on ${where}.`,
          keywords: "name avatar picture",
          settings: [
            { id: "display-name", label: "Display name" },
            { id: "avatar", label: "Avatar", keywords: "picture photo" },
          ],
        },
        ...(hasPassword(me)
          ? [
              {
                id: "password",
                label: "Password",
                icon: KeyRoundIcon,
                description: `The password you sign in to ${where} with.`,
                keywords: "security change",
              },
            ]
          : []),
      ],
    });
    groups.push({ sections: [{ id: "session", label: "Sign out", icon: LogOutIcon, danger: true, keywords: "log out remove" }] });
  }

  const known = groups.some((g) => g.sections.some((s) => s.id === open));
  const section = open && known ? open : "appearance";

  return (
    <SettingsScreen
      open={open !== null}
      onOpenChange={(next) => !next && closeSettings()}
      title="Settings"
      subtitle={me ? `Signed in to ${where}` : "On this device"}
      groups={groups}
      section={section}
      onSectionChange={setSettingsSection}
    >
      {section === "appearance" && <Appearance instanceKey={key} />}
      {section === "accessibility" && <Accessibility />}
      {section === "chat" && <Chat />}
      {section === "notifications" && <Notifications />}
      {section === "keybinds" && <Keybinds />}
      {section === "streamer" && <Streamer instanceKey={key} />}
      {section === "advanced" && <Advanced />}
      {key && section === "profile" && <Profile instanceKey={key} />}
      {key && section === "password" && <Password instanceKey={key} />}
      {key && section === "session" && <Session instanceKey={key} />}
    </SettingsScreen>
  );
}
