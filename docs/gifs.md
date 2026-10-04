# GIFs

The composer's GIF button searches a GIF library through the instance. The
instance asks the library for everyone and hands every picture back itself, so
the library never sees who searched, their address or what they sent, and apps
never load anything from it.

## Turning it on

GIFs are off until an instance admin picks a provider and saves its key, in
Instance settings › GIFs (or with `FUWA_GIF_PROVIDER` and `FUWA_GIF_API_KEY`
before the first start). The page has a "Try it" button that asks for a few
trending GIFs with what's typed, before anything is saved.

| Provider | Key | Terms to know |
| --- | --- | --- |
| GIPHY (recommended) | Free at [developers.giphy.com](https://developers.giphy.com/dashboard/): create an app, choose "API", copy the key. A new key is a rate-limited beta key; ask GIPHY for a production key once the app is up | Results must say "Powered by GIPHY"; the picker shows it, and a sent GIF says "via GIPHY" on hover. Don't change GIFs or pass them off as yours |
| Klipy | Free from Klipy's partner sign-up at [klipy.com](https://klipy.com): make an app, copy its key | Credit Klipy by results (the picker does). Klipy can mix ads into results; fuwa drops them |

Tenor's API was shut down in 2026, so it isn't offered.

Keys stay on the instance: no app gets one back, not even the settings page,
which shows only that one is saved and its last four characters.

## What leaves the instance

A search sends the provider the words typed, the page, the rating and the
key. Nothing else: no address, no account, no forwarding headers, no
language or region. Calls go straight to the provider's public address
(no redirects, private and internal addresses refused), with an 8 second
limit and at most eight at once. Searches aren't logged; a failure is logged
and reported (through the anonymous reports) by provider and kind only.

The same search within ten minutes is answered from memory, keyed by a
SHA-256 of the provider, rating, words and page. Previews in results come
through the instance's picture proxy, like link previews.

## Sending

Picking a GIF stores it on the instance once (the same GIF picked again by
anyone is the same file), with its metadata stripped and its looping kept,
and sends it as the message's GIF, so it stays after the provider drops it.
The instance signs what it hands back, so a message can only carry a GIF the
instance stored. A GIF needs Attach Files, AutoMod's picture checks look at
its first frame, and with reduce motion on it stays still until it's pointed
at or tapped. GIFs can't be sent in shared channels yet.

People can save GIFs (up to 200) and upload their own (shown under Saved).
Recently sent GIFs are kept on the device only.

## Caps

All off by default:

- Largest GIF stored: a bigger one is stored at the provider's smaller size,
  or refused when even that is too big. 32 MiB is the most ever fetched.
- Searches a minute, for each account.
- Calls to the provider a day, for the whole instance (counted in UTC days;
  answers from memory don't count).
