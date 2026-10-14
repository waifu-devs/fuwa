# App settings on every device

App settings (theme, backdrop, fonts, shortcuts, sounds...) belong to a
person, not to one browser or computer. Each app keeps them on the device
as before, so it starts at once and works offline, and keeps a copy on
every instance it's signed in to, so a new device signed in anywhere gets
them. Which instance holds them doesn't matter: each holds the whole set,
and the newest change to each setting wins.

## On the instance

`AccountService.GetAppSettings` and `SetAppSettings` keep one JSON document
per account in node.db's `app_settings`, with a `version` that goes up on
every write. A write names the version it was merged from and is refused
with `FAILED_PRECONDITION` if another device wrote meanwhile; the app reads
the document again, merges and tries again. The instance checks only that
the document is a JSON object of at most 1 MiB and never reads inside it.
It's in the account's data export and deleted with the account. Agents have
none. Apps use it only where the instance has the `app-settings-sync`
feature.

## The document

```json
{
  "format": "fuwa.app-settings.v1",
  "settings": {
    "theme": { "at": 1760400000000, "value": "yoru" },
    "keybinds": { "at": 1760400000123, "value": { "search": "Mod+K" } }
  }
}
```

Each setting is stamped `at`, unix milliseconds, when it was last changed on
some device. Merging takes, setting by setting, the one with the later
stamp (the instance's on a tie). A change made after seeing another
device's value is stamped at least one past that value's stamp, so a clock
running behind can't lose it. Settings an app doesn't know, or whose value
it can't use, stay in the document untouched for the apps that do; an app
that can't use a value keeps its own until the setting changes again.

Settings never changed on a device aren't sent: a new device takes
everything the instance has. Turning sync off on a device (its own setting,
`syncSettings`, never synced) stops it reading and writing the document.

## The settings

Names and values are the web app's (`web/src/lib/prefs.ts`'s `Prefs`);
themes and the backdrop are in `docs/themes.md`'s format, keybinds in
`web/src/lib/keybinds.ts`'s combo format. The desktop app translates its
own fields to these.

| Setting | Value |
| --- | --- |
| `theme`, `lightTheme`, `darkTheme` | a theme id |
| `followSystem` | boolean |
| `customThemes` | custom themes, as `docs/themes.md` |
| `backdrop` | a backdrop, as `docs/themes.md` |
| `density` | `"compact"`, `"default"` or `"spacious"` |
| `messageDisplay` | `"cozy"` or `"compact"` |
| `chatFontSize` | pixels, 12 to 20 |
| `font`, `codeFont` | a font pick (`lib/fonts.ts`); an imported file isn't sent |
| `reduceMotion` | `"system"`, `"always"` or `"never"` |
| `othersEffects`, `highContrast`, `underlineLinks` | boolean |
| `saturation`, `colorVisionStrength` | percent, 0 to 100 |
| `colorVision` | `lib/accessibility.ts`'s kinds |
| `roleColors` | `"names"`, `"beside"` or `"off"` |
| `language` | `"auto"` or a language tag |
| `clock` | `"auto"`, `"12h"` or `"24h"` |
| `sendWith` | `"enter"` or `"mod-enter"` |
| `notifyFor` | `"mentions"` or `"all"` |
| `unreadBadge` | boolean |
| `sounds` | `{ message, mention, join, call, ring }`, booleans |
| `volume` | percent |
| `streamerHidePersonal`, `streamerMuteSounds`, `streamerMuteNotifications` | boolean |
| `keybinds` | changed defaults: action to combo, or null for none |
| `customKeybinds` | `[{ id, action, combo }]` |
| `developerMode`, `shareReports` | boolean |
| `sendTyping` | boolean: others see you typing (docs/typing.md) |
| `inputMode` | `"voice"` or `"ptt"` |
| `pttRelease` | milliseconds |
| `userVolumes` | `"instance/user id"` to percent, up to 200 |
| `mirrorVideo`, `shareSound` | boolean |
| `cameraHeight`, `cameraFps`, `shareHeight`, `shareFps` | `lib/camera-quality.ts`, `lib/screen-share.ts` |
| `popoutName`, `popoutGlow` | boolean |
| `popoutFit` | `"cover"` or `"contain"` |
| `trustedDomains` | hosts |

The rest stays on the device, since it belongs to its screen or hardware:
the zoom, microphone, speaker and camera picks, the microphone's volume and
processing (sensitivity, echo cancellation, noise suppression, gain),
streamer mode itself, whether system notifications are on, and fonts
imported from a file. Settings only one app has (the desktop's game overlay,
its sound picks) may be added under new names; apps that don't know them
keep them for those that do.

Profile effects and decorations aren't app settings: they're part of the
profile, kept on the instance with it (docs/profile-items.md).
