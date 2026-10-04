# Capacity

How many people a fuwa instance holds online at once, what breaks first, and
the settings that decide it. The numbers come from `server/examples/load.rs`,
a load generator that starts its own instance on the machine it runs on
(never point it at a live one).

## What a box holds

The measured rows come from a 4 vCPU Intel Xeon (2.1 GHz) sandbox with 16 GB of
memory, running Ubuntu 24.04. The instance was pinned to 2 of the CPUs and the
load generator to the other 2. The extrapolated rows scale the measured CPU
and memory per person, then halve the result for headroom. Treat them as
estimates until someone measures them.

| Box | Single process | Split (per gateway) | Source |
| --- | --- | --- | --- |
| 2 vCPU, 2 GB | 6,400 online (p99 send 0.19 s, delivery 0.29 s, 1.7 GB) | 3,200 online with everything on the 2 CPUs (p99 0.57 s) | measured |
| 4 vCPU, 4 GB | about 6,000 | about 6,000 | extrapolated |
| 8 vCPU, 8 GB | about 12,000 | about 12,000 | extrapolated |
| 32 vCPU, 32 GB | about 50,000 | about 50,000 | extrapolated |

The ramp's mix is heavier than most instances will see:
- Everyone is in one home server out of 12, and also in one big server that
  everyone shares.
- A quarter of people send a message every 10 s, and a fifth of those
  messages go to the big server.
- One channel bursts at 100 messages a second.
- People join and leave.

At 6,400 online that comes to 265 messages and 329,000 deliveries a second.
Most of it is the big shared server: deliveries are messages times the people
reading them, and that's what costs CPU.

Memory is about 0.25 MB per person online in a single process. A gateway
holds about 0.19 MB per person and a shard about 0.12 MB.

### One busy server

A single server's file takes about 950 messages a second however big the box
is. Its writes go one at a time at the end, so that number comes from Turso,
not the CPU count. Past it, sending slows for that server only (p50 110 ms
at the limit), and at 512 writes waiting the server answers "busy" instead
of queueing more. Other servers on the same process carry on: their p99
stayed under 20 ms while one server ran flat out. A server that needs more
than that is a case for moving it to a shard of its own (below).

## What breaks first, and what stops it

Each of these broke a run before the fix in the same change. The before and
after numbers are from the box above.

| What | Before | After | How |
| --- | --- | --- | --- |
| A busy server's file | Turso's own checkpoint waited on writes that waited on it: writes hung and were cancelled at 30 s. The old build broke at about 2,900 online. | 6,400 online with no errors | Turso's checkpoint is off; fuwa folds the log in itself between writes once it passes 4 MB |
| Opening a connection and parsing every statement for each write | 467 messages a second for one server | 943 | Connections are kept and reused, with statements cached |
| Every write let in at once | 100 messages arriving together took 260 ms each and half the CPU; neighbours' p99 was 131 ms | 55 ms and a quarter of the CPU; neighbours' p99 was 19 ms | 4 writes run per file and up to 512 wait |
| A gateway's followers on one connection per shard | Broke at 200 online: shards take 200 streams per connection, and calls queued behind them | 3,200 online with no errors | Streams use connections of their own, 100 each |
| A sign-in flood (2,000 wrong passwords at once) | 2.9 GB, and everyone else's messages at 14 s p99 | 214 MB, 0.34 s p99; the flood gets "busy" | Half the cores check passwords and 256 wait |
| Everything at once on one server (burst, joins, uploads, a call) | Sends at 25 s, 2.3 GB, 2,479 files open, neighbours' p50 at 623 ms, errors | Sends at 170 ms p50, 228 MB, 427 files open, neighbours' p50 at 3.8 ms, no errors | All of the above |

## Settings

Caps are unlimited unless an operator sets them, with one agreed exception:
the protective limits below have finite defaults, because they stop a crash
rather than limit what people do. Each can be raised, or turned off with
`unlimited`.

| Setting | Default | What it does |
| --- | --- | --- |
| `FUWA_STREAMS_PER_ACCOUNT` | 32 (protective) | Apps and tabs one account keeps open at once, per part. An app or tab holds an events stream and a direct-message stream, each counted on its own, so 32 means 32 tabs. Against a runaway client or script. Admins can change it live in the instance settings (`streams_per_account`; unset is no limit). On a split instance it's set on the directory, which sends it to every part, and gateways count it (shards count only toward `FUWA_MAX_STREAMS`, since one client stream can open several shard streams); while a directory older than this release runs, the other parts see no limit. Past it the account is asked to close one. |
| `FUWA_MAX_STREAMS` | unlimited | People online at once on this part, for everyone. Set it from the table above to answer "this instance is full" instead of slowing down for everyone. |
| `FUWA_LIMIT_*` | unlimited | Members, channels and storage per server ([self-hosting](self-hosting.md)). |
| `FUWA_WRITE_QUEUE` | 512 (protective) | Writes one server's file may have waiting. Past it, that server's writes are told it's busy. 4 run at once, built in. |
| `FUWA_SIGN_IN_QUEUE` | 256 (protective) | Password checks that may wait. Half the cores check at once, built in. Past it, sign-ins and sign-ups are told the instance is busy. |
| Gateway streams per shard connection | 100 | Built in. |

Adding tokio worker threads (`TOKIO_WORKER_THREADS`) beyond the core count
didn't help in any run.

## Noisy neighbours

One server can't take the others down with it:
- Each server file has its own write lanes and queue, so a server sending
  flat out waits on itself.
- Each server has its own event channel. A follower who can't keep up is cut
  off and catches up from where they were; nobody else waits for them.
- Password checks, streams per account and the instance cap are shared
  limits, so no single server or account can fill them.

What one hot server still shares with its neighbours is the CPU of the part
it's on. While one server ran flat out, other servers on the same process
went from 5 ms to 6 ms p99, and to 18 ms with everything at once. Those are fine numbers, but they aren't
zero. The fix for a server that busy is to move it, not to give it more
threads. A thread of its own would still share the cores, while moving it
to another shard gives it another machine. Move it in the instance
settings' Servers page (or with `MoveServer`); its people's streams follow
it.

`/healthz/parts` shows how busy each part is (`load`): streams open, writes
running or waiting, and writes a minute. With `Authorization: Bearer
<FUWA_ADMIN_TOKEN>` it also lists the busiest servers (`busiest`), by id and
those counts only, with the shard each is on. Set `FUWA_ADMIN_TOKEN` on the
gateways to read it through them. On a split instance, each gateway adds its
own streams as `"part": "gateway", "this": true`.

## Running the load generator

```sh
cargo build --release -p fuwa-server --bin fuwa --example load
# Ramp people up until a step breaks, single process or split.
taskset -c 2,3 target/release/examples/load --mode single --server-cpus 0-1
taskset -c 2,3 target/release/examples/load --mode split --server-cpus 0-1
# One hot server next to quiet ones: calm, burst, joins, uploads, a call, everything.
taskset -c 2,3 target/release/examples/load --scenario neighbours --users 200 --server-cpus 0-1
# 2,000 wrong-password sign-ins at once.
taskset -c 2,3 target/release/examples/load --scenario flood --steps 2000 --server-cpus 0-1
```

`--help` lists the rest: steps, step length, the mix, a memory budget and a
JSON summary (`--out`). A step breaks when sending or delivery passes 1 s or
2 s at p99, when a process passes `--memory-mb`, or when calls fail. Each
process's file descriptors, memory and CPU are printed with every step.
