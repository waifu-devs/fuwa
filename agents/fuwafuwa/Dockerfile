# fuwafuwa, as Railway builds it from this folder (.railway/railway.ts).
# This folder alone: Bun runs the TypeScript as it is, with only the
# production dependencies (the SDK from npm). The base image is pinned by digest.
FROM oven/bun:1.4.4-slim@sha256:1846adf22496bd711036aae8be147e57bdebbaea2e5264e3e0b5af1db187d9a4
WORKDIR /app
ENV NODE_ENV=production
COPY package.json bun.lock ./
RUN bun install --production --frozen-lockfile && rm -rf /root/.bun/install/cache
COPY src src
USER bun
CMD ["bun", "src/main.ts"]
