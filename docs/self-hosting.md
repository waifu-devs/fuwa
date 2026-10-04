# Self-hosting fuwa

A fuwa instance is one program and one folder of data. This guide gets one
running on your own server with a domain and https, then covers updates,
backups and checking that what you run is an official build. Every setting is
listed under [Configuration](../README.md#configuration) in the README.

Pick one way to run it:

- [With Docker Compose](#with-docker-compose), with Caddy for https. The
  easiest on any Linux server.
- [With the binary](#with-the-binary), as a systemd service.
- [On Railway](#on-railway), with nothing to look after yourself.

## Before you start

**A domain.** Point a name like `chat.example.com` at your server (an `A`
record, and `AAAA` for IPv6). People use this address in the app, waifu.dev
sign-in sends people back to it, and uploaded pictures are linked through it,
so pick it before anyone signs up. It goes in `FUWA_PUBLIC_URL`.

**An encryption key** (optional, recommended). With one, every database is
encrypted on disk:

```sh
openssl rand -hex 32
```

Keep it in a password manager. Set it before fuwa starts for the first time,
and never change it: fuwa won't open data with a different key, with no key,
or with a key when the data was made without one. Lose the key and the data
is gone. When fuwa starts, its "fuwa is up" line says `encrypted=true` if the
key is in use.

**A version.** Images and binaries come from
[the releases](https://github.com/waifu-devs/fuwa/releases):

| Image tag | What it follows |
| --- | --- |
| `ghcr.io/waifu-devs/fuwa:0.1` | The newest 0.1.x release. Recommended: fixes arrive, nothing that changes how things work |
| `ghcr.io/waifu-devs/fuwa:0.1.0` | Exactly that release |
| `ghcr.io/waifu-devs/fuwa:latest` | Every change as it's merged, before it's released |
| `ghcr.io/waifu-devs/fuwa:sha-<commit>` | Exactly that commit |

Images run on x86 and ARM servers alike. The examples below use `0.1`; use the
newest release's `major.minor`.

## With Docker Compose

On a server with [Docker](https://docs.docker.com/engine/install/) and ports
80 and 443 free, make a folder with these two files.

`compose.yaml`:

```yaml
services:
  fuwa:
    image: ghcr.io/waifu-devs/fuwa:0.1
    restart: unless-stopped
    environment:
      FUWA_PUBLIC_URL: https://chat.example.com
      FUWA_NODE_NAME: Example Chat
      FUWA_ENCRYPTION_KEY: ${FUWA_ENCRYPTION_KEY}
    volumes:
      - fuwa:/data

  caddy:
    image: caddy:2
    restart: unless-stopped
    ports:
      - "80:80"
      - "443:443"
      - "443:443/udp"
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile:ro
      - caddy:/data

volumes:
  fuwa:
  caddy:
```

`Caddyfile`:

```
chat.example.com {
	reverse_proxy h2c://fuwa:8080
}
```

Put the key in a `.env` file beside them (`FUWA_ENCRYPTION_KEY=...`, readable
only by you: `chmod 600 .env`), then start it:

```sh
docker compose up -d
docker compose logs -f fuwa
```

Caddy gets a certificate for the domain by itself. `h2c://` makes it talk
HTTP/2 to fuwa, which apps that speak gRPC directly need; the web app works
either way.

Any other reverse proxy works too, if it passes requests through without
buffering responses (live updates stream) and, for gRPC apps, speaks HTTP/2 to
fuwa.

## With the binary

Download the binary for your server from
[the newest release](https://github.com/waifu-devs/fuwa/releases/latest):

```sh
curl -LO https://github.com/waifu-devs/fuwa/releases/download/v0.1.0/fuwa-0.1.0-x86_64-linux
curl -LO https://github.com/waifu-devs/fuwa/releases/download/v0.1.0/SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
sudo install -m 755 fuwa-0.1.0-x86_64-linux /usr/local/bin/fuwa
fuwa --version
```

There's `aarch64-linux` for ARM servers. The Linux binaries need glibc 2.36 or
newer (Debian 12, Ubuntu 24.04 or later); on older systems, use Docker. There
are also binaries for macOS (Apple silicon) and Windows, handy for trying fuwa
on your own computer: run it and open http://localhost:8080. (macOS won't run
one downloaded in a browser until you clear the download mark:
`xattr -d com.apple.quarantine fuwa-*-aarch64-macos`.)

Settings go in `/etc/fuwa.env` (`sudo chmod 600 /etc/fuwa.env`):

```sh
FUWA_PUBLIC_URL=https://chat.example.com
FUWA_NODE_NAME=Example Chat
FUWA_ENCRYPTION_KEY=...
# Only the proxy on this machine reaches fuwa.
FUWA_HOST=127.0.0.1
```

And a service in `/etc/systemd/system/fuwa.service`:

```ini
[Unit]
Description=fuwa chat server
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/fuwa
EnvironmentFile=/etc/fuwa.env
Environment=FUWA_DATA_PATH=/var/lib/fuwa
StateDirectory=fuwa
DynamicUser=yes
Restart=on-failure
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes

[Install]
WantedBy=multi-user.target
```

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now fuwa
journalctl -u fuwa -f
```

Then [install Caddy](https://caddyserver.com/docs/install) and give it the
same Caddyfile as above, with `h2c://127.0.0.1:8080` as the address.

## On Railway

1. In a [Railway](https://railway.com) project, add a service from the Docker
   image `ghcr.io/waifu-devs/fuwa:0.1`.
2. Attach a volume to the service, mounted at `/data`.
3. Add the variables `FUWA_PUBLIC_URL` (the address from step 5),
   `FUWA_NODE_NAME` and `FUWA_ENCRYPTION_KEY`. Railway sets `PORT`, and fuwa
   listens on it.
4. Set the health check path to `/healthz`.
5. Under networking, add your custom domain (or generate a Railway one) for
   the port fuwa listens on, `8080` unless you changed `PORT`.
6. Turn on automatic updates for the image, with the **Anytime** window, so
   new 0.1.x releases deploy by themselves once Railway notices them.

Keep it to one replica: every server's database is a file on that one volume.
For more than one machine, see [Scaling out](../README.md#scaling-out), and
for keeping communities' data in the EU or elsewhere, [Regions](regions.md).
fuwa.chat's own [Railway config](../.railway/railway.ts) is a worked example:
with its `SPLIT` setting on, the service with the domain becomes the gateways
(several replicas), a `fuwa-directory` service takes over the volume, and each
shard is a service with a volume of its own, all on Railway's private network
with `FUWA_HOST=::`.

A service with a volume can't run its old and new deployments side by side, so
with one process each deploy drops connections for a few seconds; the app
reconnects and catches up by itself. Split, only the directory and shards have
volumes, and the gateways hold calls and live streams while those restart, so
deploys go unnoticed (see [Restarts and deploys](../README.md#restarts-and-deploys)).

## After it's up

Open your address and choose **Create account**. The first account made on an
instance becomes its admin.

The cog next to the instance's name opens the instance settings, where an admin
can change, without restarting:

- the name and public address,
- who can make an account: standalone accounts, waifu.dev sign-in and single
  sign-on through your organization's identity provider (SAML or OpenID
  Connect, see [the README](../README.md#single-sign-on-saml-and-openid-connect)),
  each open, closed to new people, or off,
- who can create servers, and default limits for each one,
- the anonymous usage signal (see
  [the README](../README.md#the-anonymous-usage-signal)), on by default,
- an announcement banner, and the instance's accounts and servers.

What's set there is kept in the data folder and wins over the environment
variables, which stay as the defaults underneath.

Direct messages are end-to-end encrypted with nothing to set up: the instance
keeps only ciphertext and public keys for them (`dms.db`), and can't read them.
If a proxy in front of fuwa sets its own `Content-Security-Policy`, let scripts
use `'wasm-unsafe-eval'`, which the app's encryption needs (see
[docs/e2ee.md](e2ee.md)).

## Calls

Voice channels and calls in direct messages need one more port open to the
internet, UDP and TCP: `50000` unless you set `FUWA_MEDIA_PORT`. Sound goes
straight between the apps and that port, not through your reverse proxy.
fuwa tells apps to reach it on the machine's own address; behind NAT or in
Docker, give it the public one with `FUWA_MEDIA_ADDRESSES`.

With Docker Compose, add to the `fuwa` service:

```yaml
    ports:
      - "50000:50000/udp"
      - "50000:50000/tcp"
    environment:
      FUWA_MEDIA_ADDRESSES: 203.0.113.5   # your server's public IP (or its name)
```

With the binary, open the port in your firewall (`sudo ufw allow 50000`).

Addresses can be `HOST`, `HOST:PORT` (when a port forward changes it), or
start with `udp/` or `tcp/` for only that protocol, comma-separated. Apps use
UDP when they can and TCP on the same port when a network blocks UDP.

People on networks that block both (some offices and schools) can often
still reach port 443, so the surest fix is to give the media port 443 (map
it: `"443:50000/udp"` and `"443:50000/tcp"`, with
`FUWA_MEDIA_ADDRESSES=<public IP>:443`) on a machine where nothing else
uses that port, such as a media part on a host of its own (below).

A TURN server, such as [coturn](https://github.com/coturn/coturn) with
`use-auth-secret`, also helps, but only when the media port takes UDP:
TURN relays to it over UDP, whatever way the app reached TURN. Give fuwa
its addresses and secret, from the instance settings' **Calls** page or
`FUWA_ICE_URLS`
(`turn:turn.example.com:3478?transport=udp,turns:turn.example.com:5349`) and
`FUWA_TURN_SECRET`. Apps get a new password for each call that lasts an hour
and names no account. In coturn's config, keep the relay away from your own
network, so nobody can use it to reach machines behind it:

```
no-multicast-peers
denied-peer-ip=0.0.0.0-0.255.255.255
denied-peer-ip=10.0.0.0-10.255.255.255
denied-peer-ip=100.64.0.0-100.127.255.255
denied-peer-ip=127.0.0.0-127.255.255.255
denied-peer-ip=169.254.0.0-169.254.255.255
denied-peer-ip=172.16.0.0-172.31.255.255
denied-peer-ip=192.168.0.0-192.168.255.255
denied-peer-ip=::1
denied-peer-ip=fc00::-fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff
denied-peer-ip=fe80::-febf:ffff:ffff:ffff:ffff:ffff:ffff:ffff
```

Apps only ever connect to your instance's media port (and TURN server), never
to each other, so nobody in a call learns anyone else's address. Calls in
direct messages are end-to-end encrypted: the instance forwards sound it
can't read. `FUWA_CALLS=off` (or the Calls page) turns calls off;
`FUWA_MEDIA_PORT=off` stops this process carrying them. How calls work, and
ride out restarts, is in [calls.md](calls.md).

People with Record can record voice channels on the server: a track per
person, in `<data>/recordings/` (sealed with `FUWA_ENCRYPTION_KEY` when
it's set, and in your backups with the rest of the data directory). An
hour of one person talking is about 15 to 30 MB. `FUWA_CALL_RECORDINGS=off`
(or the Calls page) turns it off; recordings already kept stay until
someone deletes them. To bound the disk they take, cap each server's
recordings (`FUWA_LIMIT_RECORDING_STORAGE=20GB`, or per server in Settings >
Servers) and have old ones delete themselves
(`FUWA_CALL_RECORDINGS_KEEP_DAYS=30`).

On Railway, which has no public UDP, add a TCP proxy for port 50000 and set
`FUWA_MEDIA_ADDRESSES=tcp/<proxy host>:<proxy port>`. Calls then go over
TCP on the proxy's random port: they work, but networks that block unusual
ports can't join, and a TURN server can't help (it needs the media port's
UDP). For both, run the media part on a host of its own.

### The media part on a host of its own

A split instance (`FUWA_ROLE`, see the README's [Scaling out](../README.md#scaling-out)) can carry its
calls on any machine with a public IP, such as a small VPS, while
everything else stays where it is. Apps get UDP and TCP on port 443 there;
the instance's directory and shards open calls on it over HTTPS with a key
of its own (`FUWA_MEDIA_KEY`), so the machine never holds the cluster key.
It keeps nothing, so it needs no volume or backups.

1. Point a name at the machine, such as `media.example.com` (with
   Cloudflare, "DNS only": its proxy carries no calls).
2. Open ports 80 and 8443 (TCP) and 443 (UDP and TCP), and nothing else
   but SSH: `sudo ufw allow 22/tcp && sudo ufw allow 80/tcp && sudo ufw
   allow 8443/tcp && sudo ufw allow 443 && sudo ufw enable`.
3. Copy [`deploy/media-host`](../deploy/media-host) there, copy
   `.env.example` to `.env`, fill it in (`chmod 600 .env`; a new key from
   `openssl rand -hex 32`, and the image tag the rest of the instance
   runs), and run
   `docker compose up -d`. Caddy gets the certificate for port 8443 by
   itself, through port 80.
4. Check it: `curl https://media.example.com:8443/healthz` says `ok`.
5. Set `FUWA_MEDIA_URL=https://media.example.com:8443` and the same
   `FUWA_MEDIA_KEY` on the directory and every shard, in place of the old
   media part (several, comma-separated, share calls between them; with
   `FUWA_MEDIA_KEY` set they all take that key). Calls in progress join the
   new one by themselves. A media URL on the internet must be `https://`:
   fuwa refuses `http://` for anything but private names and addresses.

Nothing on it logs addresses: fuwa never does, and the Caddyfile has no
access log. Keep it on the same version as the rest of the instance: when
they update, change `FUWA_VERSION` and run `docker compose up -d`, and calls
ride the restart out as on any media part.

## Moderation providers

Servers' AutoMod can ask a moderation service about each message (the "Smart
filter" rule): TypeSafe Jev or Cloudflare Clef. Turn one on once for the whole
instance in **Instance settings, Moderation**: paste the key, press **Try a
sample scam** to check it answers, and switch it on. Every server can then pick
it with one switch and choose what happens to hate, harassment, sexual content,
violence, self-harm, scams and spam.

- **TypeSafe Jev**: an API key from your TypeSafe account. Messages go to
  `api.typesafe.ai` (US).
- **Cloudflare Clef**: an API token with the Workers AI Read permission and
  your account id. Account-owned tokens (Manage Account, Account API Tokens)
  and user tokens (My Profile, API Tokens) both work. Messages go to
  `api.cloudflare.com`.
- **Your own**: press **Add your own** and give it a name, an https address
  and, if it needs one, a key and the header it goes in. It gets the same
  requests Jev and Clef do; [automod.md](automod.md) has what fuwa sends and
  what it expects back.

You can also start with one on: `FUWA_JEV_API_KEY`, or `FUWA_CLEF_API_TOKEN`
with `FUWA_CLEF_ACCOUNT_ID`. Keys live in node.db (sealed when
`FUWA_ENCRYPTION_KEY` is set) and are never sent to apps. Only the instance
talks to the provider, and it sends the message's text alone, with mentions
and custom emoji swapped for placeholders: never who wrote it, where, or which
server. The provider bills you for what it reads. When it's slow (over 3
seconds) or down, messages go through and the servers' own rules still apply;
those failures are counted in the anonymous hourly report, by kind and
provider only.

## Updating

fuwa never updates itself. At startup and then once a day it asks GitHub
whether a newer release is out (`api.github.com`, a fixed address, with nothing
about your instance or anyone on it), and when there is one it tells you:

- the instance settings (General) show "fuwa 0.4.2 is out" to admins, with
  links to the release's notes and to this section;
- `/healthz` still answers `ok`, with a second line naming the new version,
  so a monitor that reads the body can tell you;
- the log says `a newer fuwa is out` once a day.

`FUWA_UPDATE_CHECK=off` turns the check off. Desktop apps signed in to your
instance then look for their own updates through another instance they use,
or not at all.

Back up first (see below). Then, with Docker Compose:

```sh
docker compose pull && docker compose up -d
```

The `0.1` tag in `compose.yaml` follows every 0.1.x release, so that's all a
patch release needs; a new minor version (0.2) means changing the tag. To have
patch releases go on by themselves, a container updater such as
[Watchtower](https://containrrr.dev/watchtower/) can pull and restart fuwa when
its tag moves:

```yaml
  watchtower:
    image: containrrr/watchtower
    restart: unless-stopped
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
    # Only fuwa, checked once a day at 04:00.
    command: --schedule "0 0 4 * * *" --cleanup fuwa
```

It restarts fuwa without a backup first, and each restart drops connections
for a few seconds, which the apps ride out. On Railway, the image's automatic
updates do the same (see [On Railway](#on-railway)).

With the binary, replace `/usr/local/bin/fuwa` and run
`sudo systemctl restart fuwa`. Changes to the data happen by themselves when the
new version starts. Going back to an older version afterwards isn't
supported, so restore the backup instead. Each release's notes say what's new.

### The web app and the desktop app

The web app comes inside the server, so it updates with it. Tabs that were
open during the update notice the new version, show a small "fuwa was updated"
note with a Reload button, and reload by themselves once they're in the
background or nobody has touched them for ten minutes, never with a message
half typed, in a call, or with a dialog open.

The desktop app updates itself. A little after it starts and then every six
hours it asks the first instance it can reach (yours, if it's first) for
`/updates/latest.json`, and fetches a newer build through
`/updates/files/<name>`, which your instance passes through from GitHub
(at most 16 at a time) so GitHub never sees who's updating. Your instance can't
change what it hands over: the app installs a build only when the release's
`SHA256SUMS` carries a valid signature from a key the app was built with (see
[Release signing](#release-signing)) and the file's SHA-256 matches it, and
only when it's newer than the app. People can turn "Update automatically" off
in the app's settings (Updates); it then only says a new version is out.

### Release signing

Each release's `SHA256SUMS` is signed with Waifu Devs' release key (Ed25519),
as `SHA256SUMS.sig` (the signature in base64). The public keys are in
[desktop/release-keys.txt](../desktop/release-keys.txt), one base64 line each.
To check a release by hand with OpenSSL 3:

```sh
key=$(grep -v '^#' release-keys.txt | head -1)
{ printf '\x30\x2a\x30\x05\x06\x03\x2b\x65\x70\x03\x21\x00'; echo "$key" | base64 -d; } > release-key.der
openssl pkey -pubin -inform DER -in release-key.der -out release-key.pem
base64 -d SHA256SUMS.sig > SHA256SUMS.bin
openssl pkeyutl -verify -pubin -inkey release-key.pem -rawin -in SHA256SUMS -sigfile SHA256SUMS.bin
```

The private key is only ever in the repository's `FUWA_RELEASE_SIGNING_KEY`
secret, which the Release workflow signs with. A release made without it
isn't signed, so desktop apps say it's out but won't install it.

## Backups

Everything is in the data folder (`/data` in Docker). For a copy that's
consistent, stop fuwa for the few seconds it takes:

```sh
docker compose stop fuwa
docker run --rm -v fuwa_fuwa:/data -v "$PWD":/backup busybox \
  tar czf /backup/fuwa-$(date +%F).tar.gz -C /data .
docker compose start fuwa
```

The volume is named after the folder `compose.yaml` is in (`fuwa_fuwa` for a
folder called `fuwa`; `docker volume ls` shows it). For the binary, stop the
service and copy `/var/lib/private/fuwa` instead (`/var/lib/fuwa` points to it).

Keep the encryption key somewhere other than the backups: a backup of an
encrypted instance can only be opened with it. To restore, stop fuwa, put the
files back in the empty data folder, and start it with the same key.

An admin can also save one community server as a plain SQLite file from the
instance settings (Servers). That file is never encrypted.

## Checking what you run

Every image and binary comes with a signed record of the commit and the
workflow that built it. With the [GitHub CLI](https://cli.github.com):

```sh
gh attestation verify oci://ghcr.io/waifu-devs/fuwa:0.1 --repo waifu-devs/fuwa
gh attestation verify fuwa-0.1.0-x86_64-linux --repo waifu-devs/fuwa
```

The Linux builds are also reproducible: rebuilding a release's commit gives
the same binary, byte for byte (see
[Checking the image](../README.md#checking-the-image)). The app shows the
version and commit an instance runs on its home page.

## When something's wrong

- **fuwa won't start, and the log names `FUWA_ENCRYPTION_KEY`.** The key isn't
  the one the data was made with, or the data was made without one (or with
  one, and the key is missing). Put the original back.
- **"Continue with waifu.dev" doesn't show.** It needs `FUWA_PUBLIC_URL` to be
  the https address people use.
- **Pictures don't load.** `FUWA_PUBLIC_URL` isn't the address people use;
  pictures are linked through it.
- **Messages only show up after a reload.** The reverse proxy is holding
  responses back. Caddy doesn't; for others, turn off response buffering for
  this site.
- **Anything else.** `docker compose logs fuwa` (or `journalctl -u fuwa`) says
  what happened. `FUWA_LOG=debug` says more.
