# fuwafuwa, as Railway builds it from this folder (.railway/railway.ts).
# This folder alone: Node runs the TypeScript as it is, with only the
# production dependencies (the SDK from npm). The base image is pinned by digest.
FROM node:24.21.0-bookworm-slim@sha256:d6aa754f16b3197301076f047b5def2f02ea1dbbc2ca920407d46d7ec7f87b20
WORKDIR /app
ENV NODE_ENV=production COREPACK_HOME=/opt/corepack
COPY package.json pnpm-lock.yaml ./
RUN corepack enable pnpm && pnpm install --prod --frozen-lockfile && rm -rf /root/.cache /root/.local/share/pnpm
COPY src src
USER node
CMD ["node", "src/main.ts"]
