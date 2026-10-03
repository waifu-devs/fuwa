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

People on networks that block both (some offices and schools) get through
with a TURN server, such as [coturn](https://github.com/coturn/coturn) with
`use-auth-secret`. Give fuwa its addresses and secret, from the instance
settings' **Calls** page or `FUWA_ICE_URLS`
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
`FUWA_MEDIA_ADDRESSES=tcp/<proxy host>:<proxy port>`; a TURN server elsewhere
helps people on strict networks.

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

Back up first (see below). Then, with Docker Compose:

```sh
docker compose pull && docker compose up -d
```

With the binary, replace `/usr/local/bin/fuwa` and run
`sudo systemctl restart fuwa`. Changes to the data happen by themselves when the
new version starts. Going back to an older version afterwards isn't
supported, so restore the backup instead. Each release's notes say what's new.

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
