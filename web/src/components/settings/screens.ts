import { lazyComponent } from "@/components/lazy";

/** The app's settings screens, from their own file: fetched when a settings button is about to be pressed, or on opening. */
export const SettingsScreens = lazyComponent(() => import("@/components/settings/UserSettings").then((m) => m.UserSettings));
