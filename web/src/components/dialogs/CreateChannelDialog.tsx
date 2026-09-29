import { useNavigate } from "@tanstack/react-router";
import { FolderIcon, HashIcon, LoaderCircleIcon, MegaphoneIcon } from "lucide-react";
import { useEffect, useState, type FormEvent } from "react";
import { ChannelType, createChannel } from "@/fuwa/actions";
import { useAction } from "@/fuwa/hooks";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";

const TYPES = [
  { type: ChannelType.TEXT, icon: HashIcon, label: "Text", hint: "Messages, links, Markdown" },
  { type: ChannelType.ANNOUNCEMENT, icon: MegaphoneIcon, label: "Announcements", hint: "News people follow" },
  { type: ChannelType.CATEGORY, icon: FolderIcon, label: "Category", hint: "Groups channels" },
] as const;

/** Channel names are lowercase with dashes, like the server makes them. */
const slug = (name: string) =>
  name
    .toLowerCase()
    .replace(/\s+/g, "-")
    .replace(/[^\p{L}\p{N}_-]/gu, "")
    .slice(0, 100);

export function CreateChannelDialog({
  open,
  onOpenChange,
  instanceKey,
  serverId,
  parentId = "",
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instanceKey: string;
  serverId: string;
  parentId?: string;
}) {
  const navigate = useNavigate();
  const [type, setType] = useState<ChannelType>(ChannelType.TEXT);
  const [name, setName] = useState("");
  const create = useAction(createChannel);
  const category = type === ChannelType.CATEGORY;

  useEffect(() => {
    if (open) {
      setName("");
      setType(ChannelType.TEXT);
      create.setError(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  async function submit(e: FormEvent) {
    e.preventDefault();
    const channel = await create.go(instanceKey, serverId, category ? name.trim() : slug(name), type, category ? "" : parentId);
    if (!channel) return;
    onOpenChange(false);
    if (!category) navigate({ to: "/$instance/$server/$channel", params: { instance: instanceKey, server: serverId, channel: channel.id } });
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader title="Create a channel" />
        <form onSubmit={submit} className="flex flex-col gap-4">
          <div className="grid gap-2">
            {TYPES.map((t) => (
              <button
                key={t.type}
                type="button"
                onClick={() => setType(t.type)}
                className={cn(
                  "flex items-center gap-3 rounded-2xl border p-3 text-left transition",
                  type === t.type ? "border-primary bg-primary/10" : "hover:border-primary/40",
                )}
              >
                <t.icon className={cn("size-5 transition", type === t.type ? "scale-110 text-primary" : "text-muted-foreground")} />
                <span>
                  <span className="block text-sm font-bold">{t.label}</span>
                  <span className="block text-xs text-muted-foreground">{t.hint}</span>
                </span>
              </button>
            ))}
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor="channel-name" className="font-bold">
              Name
            </Label>
            <div className="relative">
              {!category && <HashIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />}
              <Input
                id="channel-name"
                autoFocus
                required
                maxLength={100}
                placeholder={category ? "Hangout" : "new-channel"}
                value={category ? name : slug(name)}
                onChange={(e) => setName(e.target.value)}
                className={cn("h-11 rounded-xl", !category && "pl-9")}
              />
            </div>
          </div>
          {create.error && <p className="text-sm text-destructive first-letter:uppercase">{create.error}</p>}
          <Button type="submit" size="lg" disabled={create.pending || !name.trim()} className="btn h-11 rounded-xl font-bold">
            {create.pending && <LoaderCircleIcon className="animate-spin" />}
            Create {category ? "category" : "channel"}
          </Button>
        </form>
      </DialogContent>
    </Dialog>
  );
}
