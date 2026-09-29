import { Link } from "@tanstack/react-router";
import { CompassIcon, HashIcon } from "lucide-react";
import { useInstance } from "@/fuwa/hooks";
import { ConnDot, ServerIcon, connectionLabel } from "@/components/Icons";
import { useLayout } from "@/components/Shell";
import { UserPanel } from "@/components/UserPanel";

/** The sidebar on an instance's home: what it is, and your servers there. */
export function InstanceSidebar({ instanceKey }: { instanceKey: string }) {
  const inst = useInstance(instanceKey);
  const { compact, setNavOpen } = useLayout();
  if (!inst) return null;
  return (
    <>
      <header className="flex h-14 shrink-0 flex-col justify-center border-b px-4">
        <p className="truncate font-extrabold">{inst.node?.name ?? instanceKey}</p>
        <p className="flex items-center gap-1.5 truncate text-xs text-muted-foreground">
          <ConnDot state={inst.connection} /> {connectionLabel(inst.connection)} · {instanceKey}
        </p>
      </header>
      <div className="scroll-thin flex-1 overflow-y-auto p-2">
        <Link
          to="/$instance"
          params={{ instance: instanceKey }}
          activeOptions={{ exact: true }}
          onClick={() => compact && setNavOpen(false)}
          className="flex items-center gap-2 rounded-lg px-2 py-2 text-sm font-bold text-muted-foreground transition hover:bg-muted hover:text-foreground data-[status=active]:bg-primary/15 data-[status=active]:text-primary"
        >
          <CompassIcon className="size-4" /> Browse servers
        </Link>
        {inst.servers.length > 0 && (
          <p className="mt-4 mb-1 px-2 text-xs font-bold tracking-wide text-muted-foreground uppercase">Your servers</p>
        )}
        {inst.servers.map((s) => (
          <Link
            key={s.id}
            to="/$instance/$server"
            params={{ instance: instanceKey, server: s.id }}
            className="group flex items-center gap-2 rounded-lg px-2 py-1.5 text-sm transition hover:bg-muted"
          >
            <ServerIcon server={s} className="size-7 text-[0.65rem]" />
            <span className="truncate font-bold">{s.name}</span>
            <HashIcon className="ml-auto size-3.5 text-muted-foreground opacity-0 transition group-hover:opacity-100" />
          </Link>
        ))}
      </div>
      <UserPanel instanceKey={instanceKey} />
    </>
  );
}
