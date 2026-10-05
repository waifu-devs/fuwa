import { clone, create } from "@bufbuild/protobuf";
import { FilmIcon } from "lucide-react";
import { GifSettingsSchema, type InstanceSettings } from "@/gen/fuwa/v1/admin_pb";
import type { I18n } from "@/i18n/react";

export const gifsOf = (s: InstanceSettings) => s.gifs ?? create(GifSettingsSchema);

/** The instance settings this page reads: all of GIFs is one setting. */
export const GIF_FIELDS: { path: string; get: (s: InstanceSettings) => unknown; copy: (into: InstanceSettings, from: InstanceSettings) => void }[] = [
  {
    path: "gifs",
    get: (s) => {
      const g = gifsOf(s);
      // The key is never sent back: an empty field keeps the saved one.
      return [g.provider, g.apiKey.trim(), g.rating || "pg-13", g.gifBytes, g.searchesPerMinute, g.providerCallsPerDay].join("|");
    },
    copy: (into, from) => (into.gifs = clone(GifSettingsSchema, gifsOf(from))),
  },
];

/** The GIFs page in the settings menu, in the app's language. */
export const gifSection = (t: I18n["t"]) => ({
  id: "gifs",
  label: t("instancesettings.nav.gifs"),
  icon: FilmIcon,
  description: t("instancesettings.nav.gifsAbout"),
  keywords: "gif giphy klipy tenor search animated",
  settings: [
    { id: "gif-provider", label: t("instancesettings.nav.gifProvider"), keywords: "giphy klipy" },
    { id: "gif-key", label: t("instancesettings.nav.gifKey"), keywords: "api key secret" },
    { id: "gif-rating", label: t("instancesettings.nav.gifRating"), keywords: "nsfw safe content filter" },
    { id: "gif-caps", label: t("instancesettings.nav.gifCaps"), keywords: "size limit rate searches per day" },
  ],
});
