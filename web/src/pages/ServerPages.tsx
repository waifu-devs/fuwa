import { Navigate } from "@tanstack/react-router";
import { ChevronLeftIcon, HashIcon, PlusIcon } from "lucide-react";
import { useState } from "react";
import { ChannelType, Permission } from "@/gen/fuwa/v1/types_pb";
import { useAccess, useInstance } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { ChannelView } from "@/components/chat/ChannelView";
import { VoiceStage } from "@/components/calls/VoiceStage";
import { CreateChannelDialog } from "@/components/dialogs/CreateChannelDialog";
import { useLayout } from "@/components/Shell";
import { Button } from "@/components/ui/button";
import { has } from "@/lib/permissions";

/** A server with no channel picked: go to its first text channel once they're loaded. */
export function ServerIndex({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const inst = useInstance(instanceKey);
  const access = useAccess(instanceKey, serverId);
  const [creating, setCreating] = useState(false);
  const { compact, setNavOpen } = useLayout();
  // Signed out (the session ended, or an admin turned the account off): the instance's page asks to sign in.
  if (!inst || inst.connection === "signed-out") return <Navigate to="/$instance" params={{ instance: instanceKey }} replace />;
  const server = inst.servers.find((s) => s.id === serverId);
  if (!server && inst.connection === "live") return <Navigate to="/$instance" params={{ instance: instanceKey }} replace />;
  const first = inst.channels[serverId]?.find((c) => c.type === ChannelType.TEXT || c.type === ChannelType.ANNOUNCEMENT);
  if (first) return <Navigate to="/$instance/$server/$channel" params={{ instance: instanceKey, server: serverId, channel: first.id }} replace />;
  if (!inst.synced[serverId]) return <div className="shimmer m-4 h-10 rounded-xl opacity-40" />;
  return (
    <div className="grid h-full place-items-center p-6 text-center">
      {compact && (
        <button type="button" onClick={() => setNavOpen(true)} className="absolute top-2 left-2 flex items-center gap-1 rounded-full px-3 py-2 text-sm font-bold text-muted-foreground hover:bg-muted">
          <ChevronLeftIcon className="size-4" /> Channels
        </button>
      )}
      <div className="flex flex-col items-center gap-3">
        <span className="float grid size-16 place-items-center rounded-full bg-primary/15 text-primary">
          <HashIcon className="size-8" />
        </span>
        <p className="text-lg font-extrabold">No channels yet</p>
        {has(access, Permission.MANAGE_CHANNELS) ? (
          <Button onClick={() => setCreating(true)} className="btn rounded-xl font-bold">
            <PlusIcon /> Create a channel
          </Button>
        ) : (
          <p className="text-sm text-muted-foreground">None that you can see, anyway. Someone who runs the server can make one, or let you in.</p>
        )}
      </div>
      <CreateChannelDialog open={creating} onOpenChange={setCreating} instanceKey={instanceKey} serverId={serverId} />
    </div>
  );
}

export function ChannelPage({ instanceKey, serverId, channelId }: { instanceKey: string; serverId: string; channelId: string }) {
  // Only what decides the page, so the channel doesn't re-render for every event on the instance.
  const known = useFuwa((s) => !!s.instances[instanceKey]);
  const connection = useFuwa((s) => s.instances[instanceKey]?.connection);
  const channel = useFuwa((s) => s.instances[instanceKey]?.channels[serverId]?.find((c) => c.id === channelId));
  const synced = useFuwa((s) => !!s.instances[instanceKey]?.synced[serverId]);
  const joined = useFuwa((s) => !!s.instances[instanceKey]?.servers.some((x) => x.id === serverId));
  if (!known || connection === "signed-out") return <Navigate to="/$instance" params={{ instance: instanceKey }} replace />;
  if (!channel) {
    // Deleted while open, or not loaded yet.
    if (synced) return <Navigate to="/$instance/$server" params={{ instance: instanceKey, server: serverId }} replace />;
    // You left, were kicked or banned, or the server was deleted.
    if (connection === "live" && !joined) return <Navigate to="/$instance" params={{ instance: instanceKey }} replace />;
    return <div className="shimmer m-4 h-10 rounded-xl opacity-40" />;
  }
  if (channel.type === ChannelType.VOICE) return <VoiceStage instanceKey={instanceKey} serverId={serverId} channel={channel} />;
  return <ChannelView instanceKey={instanceKey} serverId={serverId} channel={channel} />;
}
