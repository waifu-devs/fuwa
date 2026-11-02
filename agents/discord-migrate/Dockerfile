# discord-migrate, the bot that copies Discord servers into fuwa servers.
# This folder alone: Bun runs the TypeScript as it is, with only the
# production dependencies (the SDK and discord.js from npm), and ffmpeg to turn
# Discord's soundboard sounds into Ogg Opus. The base image is
# pinned by digest. Keep /data on a volume so stopped migrations pick up where
# they left off.
FROM oven/bun:1.4.4-slim@sha256:1846adf22496bd711036aae8be147e57bdebbaea2e5264e3e0b5af1db187d9a4
WORKDIR /app
ENV NODE_ENV=production DATA_DIR=/data
RUN apt-get update && apt-get install -y --no-install-recommends ffmpeg && rm -rf /var/lib/apt/lists/*
COPY package.json bun.lock ./
RUN bun install --production --frozen-lockfile && rm -rf /root/.bun/install/cache \
  && mkdir /data && chown bun:bun /data
COPY src src
USER bun
CMD ["bun", "src/main.ts"]
