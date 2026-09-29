import { useNavigate } from "@tanstack/react-router";
import { ArrowLeftIcon, ArrowRightIcon, LoaderCircleIcon, ServerIcon as ServerGlyph, SparklesIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState, type FormEvent } from "react";
import type { Node } from "@/gen/fuwa/v1/types_pb";
import { probe, run, signIn, signUp } from "@/fuwa/actions";
import { useAction } from "@/fuwa/hooks";
import { instanceKey } from "@/fuwa/saved";
import { AutoHeight } from "@/components/animate-ui/primitives/effects/auto-height";
import { Tabs, TabsContent, TabsContents, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";

const EASE = [0.22, 1, 0.36, 1] as const;

/**
 * Finds a fuwa server by its address, then signs in or creates an account
 * there. Used on the welcome screen and in the "add a server" dialog.
 */
export function Connect({ initialUrl, onDone }: { initialUrl?: string; onDone?: (key: string) => void }) {
  const [found, setFound] = useState<{ url: string; node: Node } | null>(null);
  return (
    <AutoHeight>
      <AnimatePresence mode="wait" initial={false}>
        {found ? (
          <motion.div
            key="account"
            initial={{ opacity: 0, x: 24 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: -24 }}
            transition={{ duration: 0.3, ease: EASE }}
          >
            <Account url={found.url} node={found.node} onBack={() => setFound(null)} onDone={onDone} />
          </motion.div>
        ) : (
          <motion.div
            key="where"
            initial={{ opacity: 0, x: -24 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: 24 }}
            transition={{ duration: 0.3, ease: EASE }}
          >
            <Where initialUrl={initialUrl} onFound={setFound} />
          </motion.div>
        )}
      </AnimatePresence>
    </AutoHeight>
  );
}

/** When this page is served by a fuwa server, that server is the obvious first choice. */
function useHomeInstance() {
  const [home, setHome] = useState<{ url: string; node: Node } | null>(null);
  useEffect(() => {
    let cancelled = false;
    run(probe(window.location.origin))
      .then((found) => !cancelled && setHome(found))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);
  return home;
}

function Where({ initialUrl, onFound }: { initialUrl?: string; onFound: (f: { url: string; node: Node }) => void }) {
  const [address, setAddress] = useState(initialUrl ?? "");
  const [shake, setShake] = useState(0);
  const lookup = useAction(probe);
  const home = useHomeInstance();

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!address.trim()) return;
    const found = await lookup.go(address);
    if (found) onFound(found);
    else setShake((n) => n + 1);
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-4">
      <div className="flex flex-col gap-2">
        <Label htmlFor="address" className="font-bold">
          Server address
        </Label>
        <div key={shake} className={cn("relative", shake > 0 && "shake")}>
          <ServerGlyph className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            id="address"
            autoFocus
            inputMode="url"
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            placeholder="chat.example.com"
            value={address}
            onChange={(e) => setAddress(e.target.value)}
            aria-invalid={!!lookup.error}
            className="h-11 rounded-xl pl-9 text-base"
          />
        </div>
        <AnimatePresence>
          {lookup.error && (
            <motion.p
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: "auto" }}
              exit={{ opacity: 0, height: 0 }}
              className="text-sm text-destructive"
            >
              Couldn't find a fuwa server there: {lookup.error}.
            </motion.p>
          )}
        </AnimatePresence>
      </div>

      <AnimatePresence>
        {home && (
          <motion.button
            type="button"
            layout
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            onClick={() => onFound(home)}
            className="group card-pop flex items-center gap-3 rounded-2xl border bg-background/60 p-3 text-left"
          >
            <span className="grid size-10 place-items-center rounded-xl bg-primary/15 text-primary">
              <SparklesIcon className="size-5 transition group-hover:rotate-12 group-hover:scale-110" />
            </span>
            <span className="min-w-0 flex-1">
              <span className="block truncate font-bold">{home.node.name}</span>
              <span className="block truncate text-xs text-muted-foreground">{instanceKey(home.url)} · this server</span>
            </span>
            <ArrowRightIcon className="size-4 text-muted-foreground transition group-hover:translate-x-1 group-hover:text-primary" />
          </motion.button>
        )}
      </AnimatePresence>

      <Button type="submit" size="lg" disabled={lookup.pending || !address.trim()} className="btn h-11 rounded-xl font-bold">
        {lookup.pending ? <LoaderCircleIcon className="animate-spin" /> : null}
        {lookup.pending ? "Looking…" : "Continue"}
      </Button>
      <p className="text-center text-xs text-muted-foreground">
        Any fuwa server works: ours, a friend's, or <a className="font-bold text-primary hover:underline" href="https://github.com/waifu-devs/fuwa#self-host" target="_blank" rel="noreferrer">one you run</a>.
      </p>
    </form>
  );
}

function Account({
  url,
  node,
  onBack,
  onDone,
}: {
  url: string;
  node: Node;
  onBack: () => void;
  onDone?: (key: string) => void;
}) {
  const navigate = useNavigate();
  const methods = node.auth;
  const canSignUp = !!methods?.localSignUp;
  const canSignIn = !!methods?.localSignIn;
  const [tab, setTab] = useState(canSignIn ? "sign-in" : "sign-up");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [shake, setShake] = useState(0);
  const inAction = useAction(signIn);
  const upAction = useAction(signUp);
  const action = tab === "sign-in" ? inAction : upAction;

  async function submit(e: FormEvent) {
    e.preventDefault();
    const key =
      tab === "sign-in"
        ? await inAction.go(url, username.trim().toLowerCase(), password)
        : await upAction.go(url, username.trim().toLowerCase(), password, displayName.trim());
    if (!key) {
      setShake((n) => n + 1);
      return;
    }
    if (onDone) onDone(key);
    else navigate({ to: "/$instance", params: { instance: key } });
  }

  if (!canSignIn && !canSignUp) {
    return (
      <div className="flex flex-col gap-4">
        <Header url={url} node={node} onBack={onBack} />
        <p className="rounded-2xl bg-muted p-4 text-sm text-muted-foreground">
          This server doesn't take sign-ins with a username and password. Linked waifu.dev accounts are coming soon.
        </p>
      </div>
    );
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-4">
      <Header url={url} node={node} onBack={onBack} />
      <Tabs value={tab} onValueChange={setTab}>
        <TabsList className="w-full">
          <TabsTrigger value="sign-in" disabled={!canSignIn}>
            Sign in
          </TabsTrigger>
          <TabsTrigger value="sign-up" disabled={!canSignUp}>
            Create account
          </TabsTrigger>
        </TabsList>
        <TabsContents>
          <TabsContent value="sign-in" />
          <TabsContent value="sign-up">
            <div className="flex flex-col gap-2 pt-2">
              <Label htmlFor="display-name" className="font-bold">
                Display name
              </Label>
              <Input
                id="display-name"
                placeholder="What people see"
                value={displayName}
                onChange={(e) => setDisplayName(e.target.value)}
                maxLength={64}
                className="h-11 rounded-xl"
              />
            </div>
          </TabsContent>
        </TabsContents>
      </Tabs>
      <div key={`u${shake}`} className={cn("flex flex-col gap-2", shake > 0 && "shake")}>
        <Label htmlFor="username" className="font-bold">
          Username
        </Label>
        <Input
          id="username"
          autoFocus
          autoComplete="username"
          autoCapitalize="none"
          spellCheck={false}
          placeholder="lowercase letters, numbers, . and _"
          value={username}
          onChange={(e) => setUsername(e.target.value)}
          className="h-11 rounded-xl"
          required
        />
        <Label htmlFor="password" className="mt-2 font-bold">
          Password
        </Label>
        <Input
          id="password"
          type="password"
          autoComplete={tab === "sign-in" ? "current-password" : "new-password"}
          placeholder={tab === "sign-in" ? "" : "at least 8 characters"}
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          className="h-11 rounded-xl"
          required
        />
      </div>
      <AnimatePresence>
        {action.error && (
          <motion.p
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            className="text-sm text-destructive first-letter:uppercase"
          >
            {action.error}
          </motion.p>
        )}
      </AnimatePresence>
      <Button type="submit" size="lg" disabled={action.pending} className="btn h-11 rounded-xl font-bold">
        {action.pending ? <LoaderCircleIcon className="animate-spin" /> : null}
        {tab === "sign-in" ? "Sign in" : "Create account"}
      </Button>
    </form>
  );
}

function Header({ url, node, onBack }: { url: string; node: Node; onBack: () => void }) {
  return (
    <div className="flex items-center gap-3">
      <button
        type="button"
        onClick={onBack}
        className="grid size-9 place-items-center rounded-full text-muted-foreground transition hover:-translate-x-0.5 hover:bg-muted hover:text-foreground"
        aria-label="Pick another server"
      >
        <ArrowLeftIcon className="size-4" />
      </button>
      <div className="min-w-0">
        <p className="truncate font-extrabold">{node.name}</p>
        <p className="truncate text-xs text-muted-foreground">
          {instanceKey(url)} · fuwa {node.version}
        </p>
      </div>
    </div>
  );
}
