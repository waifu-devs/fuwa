# discord-migrate, the bot that copies Discord servers into fuwa servers.
# This folder alone: Node runs the TypeScript as it is, with only the
# production dependencies (the SDK and discord.js from npm), and ffmpeg to turn
# Discord's soundboard sounds into Ogg Opus. The base image is
# pinned by digest. Keep /data on a volume so stopped migrations pick up where
# they left off.
FROM node:24.21.0-bookworm-slim@sha256:d6aa754f16b3197301076f047b5def2f02ea1dbbc2ca920407d46d7ec7f87b20
WORKDIR /app
ENV NODE_ENV=production COREPACK_HOME=/opt/corepack DATA_DIR=/data
RUN apt-get update && apt-get install -y --no-install-recommends ffmpeg && rm -rf /var/lib/apt/lists/*
COPY package.json pnpm-lock.yaml ./
RUN corepack enable pnpm && pnpm install --prod --frozen-lockfile && rm -rf /root/.cache /root/.local/share/pnpm \
  && mkdir /data && chown node:node /data
COPY src src
USER node
CMD ["node", "src/main.ts"]
