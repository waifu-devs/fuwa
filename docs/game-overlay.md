# The game overlay and push to talk

The desktop app's overlay over the game you're playing, like Discord's, and
push to talk that works with the game in front. The web app has no overlay,
and its push to talk works only while its tab has focus: a browser can't
draw over other programs or hear keys meant for them.

## What people see

- In a corner of the screen (Settings → Game overlay picks which), over the
  game: the call you're in (where it is, who's in it, each lit up while they
  talk, who's muted or deafened) and messages that would notify you, for a
  few seconds each. Every click goes through to the game.
- Its key, Shift+` unless changed ("Use the game overlay" on the Keyboard
  page), puts it in use: the screen dims, the cards take clicks (mute,
  deafen, hang up, open a message in fuwa) and the key again, Esc or a click
  outside the cards puts it back.
- It shows "In games" (the default): while a program covers its screen, or
  a game reports what you're playing (`core::presence`). Or "In calls":
  whenever you're in a call. Never while fuwa itself is in front.
- How solid it is, and whether only the people talking show, are settings
  too. With "Messages in the overlay" off, messages go to the system's
  notifications as before. Do not disturb and streamer mode keep it quiet as
  they do the rest of the app.

## How it works

- `desktop/src/ui/game_overlay.rs` is a second GPUI window, a view of the
  core like the main one: transparent, the size of the primary display, the
  pop-up kind (above other windows), opened without taking focus. The main
  window opens and closes it (`FuwaApp::sync_overlay`) as calls, the window
  in front and the settings change.
- `desktop/src/ui/click_through.rs` makes it let clicks through, which GPUI
  can't, through the system's window under GPUI's: `WS_EX_LAYERED |
  WS_EX_TRANSPARENT` on Windows, `ignoresMouseEvents` on macOS, an empty
  input shape on X11. It also puts the overlay back on top every 2 seconds.
- `desktop/src/core/foreground.rs` tells whether another program covers its
  screen: the window in front and its monitor on Windows, the window
  manager's active window and its full-screen state on X11. macOS tells
  apps nothing about other apps' windows without Screen Recording, so there
  only games that report what you play count. It answers yes or no and
  keeps nothing.
- `desktop/src/core/hotkeys.rs` hears keys while another program is in
  front: push to talk, the overlay's key, and mute and deafen during a call.
  It hooks nothing. A thread reads whether the watched keys are down, 100
  times a second, while something is watched (`GetAsyncKeyState`,
  `CGEventSourceKeyState`, X11's `QueryKeymap`), so the game still gets
  every key and no other key is read. With a fuwa window in front, the
  window's own shortcuts act instead, except push to talk, which the
  thread always handles so it keeps going across a switch to the game.

## Where it can't

- Wayland lets no program see keys meant for another or draw over it, so
  the overlay isn't offered there and push to talk works only while fuwa is
  in front (the Voice page says so).
- macOS hears keys only once fuwa is allowed under Input Monitoring; the
  Voice and Game overlay pages say so and open System Settings.
- Games in exclusive full screen draw over every window, the overlay too.
  It works over borderless and windowed games. Drawing inside a game's own
  frames (hooking DirectX or Vulkan, as Discord does) is left out: it's
  Windows only, and anti-cheat refuses it.
- It covers the primary display only.
