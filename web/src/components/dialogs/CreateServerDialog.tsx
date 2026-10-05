import { useNavigate } from "@tanstack/react-router";
import { LoaderCircleIcon, MapPinIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useState, type FormEvent } from "react";
import { ServerCreation } from "@/gen/fuwa/v1/types_pb";
import { createServer } from "@/fuwa/actions";
import { useAction, useInstances } from "@/fuwa/hooks";
import { ServerIcon } from "@/components/Icons";
import { PictureField } from "@/components/PictureField";
import { Private } from "@/components/Private";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { hasRegions, regionMark } from "@/lib/regions";
import { useI18n } from "@/i18n/react";
import { cn } from "@/lib/utils";

export function CreateServerDialog({
  open,
  onOpenChange,
  defaultInstance,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  defaultInstance?: string;
}) {
  const { t } = useI18n();
  const instances = useInstances().filter((i) => i.me);
  // Kept between openings, unless it's opened for a particular fuwa server.
  const [where, setWhere] = useState(defaultInstance ?? instances[0]?.key ?? "");
  // Each opening starts a blank form.
  const [opened, setOpened] = useState(0);
  const [wasOpen, setWasOpen] = useState(open);
  if (wasOpen !== open) {
    setWasOpen(open);
    if (open) {
      setOpened((n) => n + 1);
      if (defaultInstance && instances.some((i) => i.key === defaultInstance)) setWhere(defaultInstance);
    }
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader title={t("workspace.createServer.title")} description={t("workspace.createServer.about")} />
        <CreateServerForm key={opened} onOpenChange={onOpenChange} instances={instances} where={where} setWhere={setWhere} />
      </DialogContent>
    </Dialog>
  );
}

function CreateServerForm({
  onOpenChange,
  instances,
  where,
  setWhere,
}: {
  onOpenChange: (open: boolean) => void;
  instances: Instance[];
  where: string;
  setWhere: (key: string) => void;
}) {
  const { t } = useI18n();
  const navigate = useNavigate();
  const { inst, pick, iconUrl, setIconUrl, regions, picked, setRegion } = usePlace(instances, where, setWhere);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [discoverable, setDiscoverable] = useState(false);
  const create = useAction(createServer);
  const blocked = creationBlocked(inst) ? t("workspace.createServer.blocked") : null;

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!inst) return;
    const server = await create.go(inst.key, name.trim(), description.trim(), discoverable, iconUrl, hasRegions(regions) && !picked?.home ? (picked?.id ?? "") : "");
    if (!server) return;
    onOpenChange(false);
    navigate({ to: "/$instance/$server", params: { instance: inst.key, server: server.id } });
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-4">
      <div className="flex items-center gap-4">
        {inst && (
          <PictureField
            compact
            id="server-icon"
            instanceKey={inst.key}
            kind="icon"
            value={iconUrl}
            onChange={setIconUrl}
            disabled={!!blocked}
            fallback={<IconPreview name={name} />}
          />
        )}
        <div className="flex min-w-0 flex-1 flex-col gap-2">
          <Label htmlFor="server-name" className="font-bold">
            {t("workspace.createServer.name")}
          </Label>
          <Input
            id="server-name"
            autoFocus
            required
            maxLength={100}
            placeholder={t("workspace.createServer.namePlaceholder")}
            value={name}
            onChange={(e) => setName(e.target.value)}
            className="h-11 rounded-xl"
          />
        </div>
      </div>
      <div className="flex flex-col gap-2">
        <Label htmlFor="server-description" className="font-bold">
          {t("workspace.createServer.description")} <span className="font-normal text-muted-foreground">{t("workspace.createServer.optional")}</span>
        </Label>
        <Textarea
          id="server-description"
          rows={2}
          maxLength={1000}
          placeholder={t("workspace.createServer.descriptionPlaceholder")}
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          className="rounded-xl"
        />
      </div>
      {instances.length > 1 && <InstanceChoice instances={instances} current={inst?.key} onPick={pick} />}
      <RegionChoice instanceKey={inst?.key} regions={regions} picked={picked} onPick={setRegion} />
      <BrowseToggle on={discoverable} onChange={setDiscoverable} instanceName={inst?.node?.name} />
      <AnimatePresence>
        {(create.error || blocked) && (
          <motion.p initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} className="text-sm text-destructive first-letter:uppercase">
            {blocked ?? create.error}
          </motion.p>
        )}
      </AnimatePresence>
      <Button type="submit" size="lg" disabled={create.pending || !name.trim() || !!blocked || !inst} className="btn h-11 rounded-xl font-bold">
        {create.pending && <LoaderCircleIcon className="animate-spin" />}
        {t("workspace.createServer.submit")}
      </Button>
    </form>
  );
}

/** Which fuwa server it goes on, with the icon and region picked for that one. */
function usePlace(instances: Instance[], where: string, setWhere: (key: string) => void) {
  // An uploaded icon lives on the fuwa server it went to, and regions are that server's: both count only there.
  const [icon, setIcon] = useState<{ on?: string; url: string }>({ url: "" });
  const [region, setRegionOn] = useState<{ on?: string; id: string }>({ id: "" });
  const inst = instances.find((i) => i.key === where) ?? instances[0];
  const regions = inst?.node?.regions ?? [];
  // The home region unless another was picked on this fuwa server.
  const picked = (region.on === inst?.key ? regions.find((r) => r.id === region.id) : undefined) ?? regions.find((r) => r.home) ?? regions[0];
  return {
    inst,
    // Another fuwa server starts over on icon and region.
    pick: (key: string) => {
      setWhere(key);
      setIcon({ url: "" });
      setRegionOn({ id: "" });
    },
    iconUrl: icon.on === inst?.key ? icon.url : "",
    setIconUrl: (url: string) => setIcon({ on: inst?.key, url }),
    regions,
    picked,
    setRegion: (id: string) => setRegionOn({ on: inst?.key, id }),
  };
}

/** The icon until one is uploaded: the name's first letter, bouncing as it changes. */
function IconPreview({ name }: { name: string }) {
  return (
    <motion.span key={name.trim().slice(0, 1) || "empty"} initial={{ scale: 0.8, rotate: -8 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 500, damping: 15 }} className="block size-full">
      <ServerIcon server={{ id: name || "new", name: name || "?", iconUrl: "" }} active className="size-full text-xl" />
    </motion.span>
  );
}

/** Whether it shows in Browse on its fuwa server. */
function BrowseToggle({ on, onChange, instanceName }: { on: boolean; onChange: (on: boolean) => void; instanceName: string | undefined }) {
  const { t } = useI18n();
  return (
    <label className="flex cursor-pointer items-center justify-between gap-4 rounded-2xl border p-3">
      <span>
        <span className="block text-sm font-bold">{t("workspace.createServer.browse")}</span>
        <span className="block text-xs text-muted-foreground">
          {on ? t("workspace.createServer.browseOn", { instance: instanceName ?? t("workspace.createServer.thisInstance") }) : t("workspace.createServer.browseOff")}
        </span>
      </span>
      <Switch checked={on} onCheckedChange={onChange} />
    </label>
  );
}

type Instance = ReturnType<typeof useInstances>[number];
type Region = NonNullable<Instance["node"]>["regions"][number];

/** Whether this fuwa server lets you make servers: not when it's off, or kept for its admins and you aren't one. */
const creationBlocked = (inst: Instance | undefined) => {
  const policy = inst?.node?.serverCreation;
  return policy === ServerCreation.DISABLED || (policy === ServerCreation.ADMINS && !inst?.admin);
};

/** Which of your fuwa servers the new one lives on. */
function InstanceChoice({ instances, current, onPick }: { instances: Instance[]; current: string | undefined; onPick: (key: string) => void }) {
  const { t } = useI18n();
  return (
    <div className="flex flex-col gap-2">
      <Label className="font-bold">{t("workspace.createServer.livesOn")}</Label>
      <div className="flex flex-wrap gap-2">
        {instances.map((i) => (
          <button
            key={i.key}
            type="button"
            onClick={() => onPick(i.key)}
            className={cn(
              "rounded-full border px-3 py-1.5 text-sm font-bold transition",
              i.key === current ? "border-primary bg-primary/15 text-primary" : "hover:border-primary/50",
            )}
          >
            {i.node?.name ?? <Private text={i.key} />}
          </button>
        ))}
      </div>
    </div>
  );
}

/** Where in the world it's kept, on fuwa servers with more than one region. */
function RegionChoice({ instanceKey, regions, picked, onPick }: { instanceKey: string | undefined; regions: Region[]; picked: Region | undefined; onPick: (id: string) => void }) {
  const { t } = useI18n();
  return (
    <AnimatePresence initial={false}>
      {hasRegions(regions) && (
        <motion.div
          key={instanceKey}
          initial={{ opacity: 0, height: 0 }}
          animate={{ opacity: 1, height: "auto" }}
          exit={{ opacity: 0, height: 0 }}
          transition={{ type: "spring", stiffness: 420, damping: 34 }}
          className="flex flex-col gap-2 overflow-hidden"
        >
          <Label className="font-bold">{t("workspace.createServer.region")}</Label>
          <div role="radiogroup" aria-label={t("workspace.createServer.region")} className="flex flex-wrap gap-2">
            {regions.map((r) => {
              const on = r.id === picked?.id;
              return (
                <button
                  key={r.id}
                  type="button"
                  role="radio"
                  aria-checked={on}
                  onClick={() => onPick(r.id)}
                  className={cn("relative flex items-center gap-2 rounded-full border py-1.5 pr-3 pl-1.5 text-sm font-bold transition-colors", on ? "border-primary text-primary" : "hover:border-primary/50")}
                >
                  {on && <motion.span layoutId="create-server-region" transition={{ type: "spring", stiffness: 500, damping: 35 }} className="absolute inset-0 rounded-full bg-primary/15" />}
                  <span className={cn("relative grid size-6 place-items-center rounded-full text-[0.6rem] font-extrabold transition-colors", on ? "bg-primary text-primary-foreground" : "bg-muted text-muted-foreground")}>
                    {regionMark(r.name)}
                  </span>
                  <span className="relative">{r.name}</span>
                  {r.home && <span className="relative text-xs font-normal text-muted-foreground">{t("workspace.createServer.home")}</span>}
                </button>
              );
            })}
          </div>
          <AnimatePresence mode="popLayout" initial={false}>
            <motion.p
              key={picked?.id}
              initial={{ opacity: 0, y: 4 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -4 }}
              transition={{ duration: 0.18 }}
              className="flex items-start gap-1.5 text-xs text-muted-foreground"
            >
              <MapPinIcon className="mt-px size-3.5 shrink-0 text-primary" />
              {t("workspace.createServer.regionNote", { region: picked?.name ?? "", home: regions.find((r) => r.home)?.name ?? t("workspace.createServer.homeRegion") })}
            </motion.p>
          </AnimatePresence>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
