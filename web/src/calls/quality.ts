import { useEffect, useSyncExternalStore, type RefObject } from "react";

/**
 * How your connection to the call's media server is doing: ping, packet
 * loss, jitter and the route, read from WebRTC's own stats every couple of
 * seconds. Only the overall level (good, okay, poor) goes through React;
 * the numbers are written straight into the elements showing them, so a
 * new sample re-renders nothing.
 */

export type Level = "good" | "okay" | "poor";
/** udp: straight to the media server. tcp: UDP was blocked. relay: through a TURN server. */
export type Route = "udp" | "tcp" | "relay";

export type Quality = {
  /** Round trip to the media server, ms. */
  ping: number | null;
  /** Share of sound lost on the way, 0 to 1, either direction. */
  loss: number;
  /** ms. */
  jitter: number;
  route: Route | null;
  level: Level | null;
  /** The last minute of pings, oldest first. */
  history: number[];
};

const EVERY_MS = 2_000;
const HISTORY = 30;

const EMPTY: Quality = { ping: null, loss: 0, jitter: 0, route: null, level: null, history: [] };
let quality: Quality = EMPTY;
const listeners = new Set<() => void>();

function set(next: Quality) {
  quality = next;
  for (const l of listeners) l();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

export const getQuality = () => quality;

export function levelOf(ping: number, loss: number): Level {
  if (ping >= 300 || loss >= 0.1) return "poor";
  if (ping >= 120 || loss >= 0.02) return "okay";
  return "good";
}

/** The level, which is all a component re-renders for. */
export const useQualityLevel = () => useSyncExternalStore(subscribe, () => quality.level);

/** Writes something from each sample into an element, without rendering. */
export function useQualityText(ref: RefObject<HTMLElement | null>, text: (q: Quality) => string) {
  useEffect(() => {
    const write = () => {
      const t = text(quality);
      if (ref.current && ref.current.textContent !== t) ref.current.textContent = t;
    };
    write();
    return subscribe(write);
  }, [ref, text]);
}

/** Calls `draw` with each sample, for things like the ping graph. */
export function useQualityEffect(draw: (q: Quality) => void) {
  useEffect(() => {
    draw(quality);
    return subscribe(() => draw(quality));
  }, [draw]);
}

export const formatPing = (q: Quality) => (q.ping === null ? "–" : `${q.ping} ms`);

type Totals = { lost: number; received: number };

/** What one look at the stats found. */
export function read(report: RTCStatsReport, before: Totals | null): { ping: number | null; loss: number; jitter: number; route: Route | null; totals: Totals } {
  type Stat = Record<string, unknown> & { type: string; id: string };
  const all = [...report.values()] as Stat[];
  const byId = new Map(all.map((s) => [s.id, s]));
  const transport = all.find((s) => s.type === "transport");
  const pair: Stat | undefined =
    (transport?.selectedCandidatePairId ? byId.get(String(transport.selectedCandidatePairId)) : undefined) ??
    all.find((s) => s.type === "candidate-pair" && (s.selected === true || (s.nominated === true && s.state === "succeeded")));

  let ping: number | null = null;
  let route: Route | null = null;
  if (pair) {
    const rtt = typeof pair.currentRoundTripTime === "number" ? pair.currentRoundTripTime : null;
    if (rtt !== null) ping = Math.round(rtt * 1000);
    const local = byId.get(String(pair.localCandidateId));
    const remote = byId.get(String(pair.remoteCandidateId));
    if (local?.candidateType === "relay") route = "relay";
    else if (remote?.protocol === "tcp" || local?.protocol === "tcp") route = "tcp";
    else if (local || remote) route = "udp";
  }

  // Sound coming in: what went missing since the last look.
  let lost = 0;
  let received = 0;
  const jitters: number[] = [];
  let outLoss = 0;
  for (const s of all) {
    if (s.type === "inbound-rtp" && s.kind === "audio") {
      lost += Number(s.packetsLost) || 0;
      received += Number(s.packetsReceived) || 0;
      if (typeof s.jitter === "number") jitters.push(s.jitter);
    } else if (s.type === "remote-inbound-rtp" && s.kind === "audio") {
      // Sound going out, as the media server reported it.
      if (typeof s.fractionLost === "number") outLoss = Math.max(outLoss, s.fractionLost);
      if (typeof s.jitter === "number") jitters.push(s.jitter);
      if (ping === null && typeof s.roundTripTime === "number") ping = Math.round(s.roundTripTime * 1000);
    }
  }
  const totals = { lost, received };
  let inLoss = 0;
  if (before) {
    const dLost = Math.max(0, lost - before.lost);
    const dReceived = Math.max(0, received - before.received);
    if (dLost + dReceived > 0) inLoss = dLost / (dLost + dReceived);
  }
  const jitter = jitters.length ? Math.round(Math.max(...jitters) * 1000) : 0;
  return { ping, loss: Math.max(inLoss, outLoss), jitter, route, totals };
}

/** Samples the call's connection until the returned function is called. */
export function watchQuality(connection: () => RTCPeerConnection | null): () => void {
  let pc: RTCPeerConnection | null = null;
  let totals: Totals | null = null;
  let stopped = false;
  const sample = async () => {
    const now = connection();
    if (now !== pc) {
      // Joined again: counts start over, and the old numbers no longer say anything.
      pc = now;
      totals = null;
      set({ ...quality, ping: null, level: null });
    }
    if (!pc || pc.connectionState !== "connected") return;
    let report: RTCStatsReport;
    try {
      report = await pc.getStats();
    } catch {
      return;
    }
    if (stopped || pc !== connection()) return;
    const found = read(report, totals);
    totals = found.totals;
    const ping = found.ping ?? quality.ping;
    const history = ping === null ? quality.history : [...quality.history, ping].slice(-HISTORY);
    set({
      ping,
      loss: found.loss,
      jitter: found.jitter,
      route: found.route ?? quality.route,
      level: ping === null ? null : levelOf(ping, found.loss),
      history,
    });
  };
  void sample();
  const timer = setInterval(() => void sample(), EVERY_MS);
  return () => {
    stopped = true;
    clearInterval(timer);
    set(EMPTY);
  };
}
