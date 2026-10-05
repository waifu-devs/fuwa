import { useNavigate } from "@tanstack/react-router";
import {
  ArrowLeftIcon,
  ArrowRightIcon,
  BuildingIcon,
  Flower2Icon,
  KeyRoundIcon,
  LoaderCircleIcon,
  ServerIcon as ServerGlyph,
  ShieldCheckIcon,
  SparklesIcon,
  GlobeIcon,
} from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import type { Node } from "@/gen/fuwa/v1/types_pb";
import { probe, run, signIn, signUp, startLinkedSignIn, startProviderSignIn, startSsoSignIn, verifyTwoFactor } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAction } from "@/fuwa/hooks";
import { instanceKey } from "@/fuwa/saved";
import { AutoHeight } from "@/components/animate-ui/primitives/effects/auto-height";
import { BuildLabel } from "@/components/BuildLabel";
import { MotionButton } from "@/components/motion-button";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { CodeInput } from "@/components/CodeInput";
import { Private, usePrivateField } from "@/components/Private";
import { Tabs, TabsContent, TabsContents, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { issuerName } from "@/lib/linked";
import { HostedBadge } from "@/components/HostedBadge";
import { ProviderMark } from "@/components/ProviderMarks";
import { T, useI18n } from "@/i18n/react";
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
  const privateField = usePrivateField();
  const { t } = useI18n();

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
          {t("connect.where.address")}
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
            className={cn("h-11 rounded-xl pl-9 text-base", privateField)}
          />
        </div>
        <AnimatePresence mode="popLayout">
          {lookup.error && (
            <motion.p
              {...SLIDE_IN}
              transition={SPRING}
              className="text-sm text-destructive"
            >
              {t("connect.where.notFound", { problem: lookup.error })}
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
              <span className="flex min-w-0 items-center gap-2">
                <span className="truncate font-bold">{home.node.name}</span>
                <HostedBadge url={home.url} variant="still" />
              </span>
              <span className="block truncate text-xs text-muted-foreground">
                <T k="connect.where.thisServer" values={{ address: <Private text={instanceKey(home.url)} /> }} />
              </span>
            </span>
            <ArrowRightIcon className="size-4 text-muted-foreground transition group-hover:translate-x-1 group-hover:text-primary" />
          </motion.button>
        )}
      </AnimatePresence>

      <MotionButton layout="position" transition={SPRING} type="submit" size="lg" disabled={lookup.pending || !address.trim()} className="btn h-11 rounded-xl font-bold">
        {lookup.pending ? <LoaderCircleIcon className="animate-spin" /> : null}
        {lookup.pending ? t("connect.where.looking") : t("common.continue")}
      </MotionButton>
      <motion.p layout="position" transition={SPRING} className="text-center text-xs text-muted-foreground">
        <T
          k="connect.where.anyServer"
          values={{
            link: (
              <a className="font-bold text-primary hover:underline" href="https://github.com/waifu-devs/fuwa/blob/master/docs/self-hosting.md" target="_blank" rel="noreferrer">
                {t("connect.where.anyServerLink")}
              </a>
            ),
          }}
        />
      </motion.p>
    </form>
  );
}

/**
 * Signs in or creates an account on an instance already found. Without
 * `onBack` there's no going back to pick another. Signing in with waifu.dev
 * leaves the page, so it comes back to `returnTo` (or the instance's home)
 * instead of calling `onDone`.
 */
export function Account({
  url,
  node,
  onBack,
  onDone,
  returnTo,
}: {
  url: string;
  node: Node;
  onBack?: () => void;
  onDone?: (key: string) => void;
  returnTo?: string;
}) {
  const navigate = useNavigate();
  const { t } = useI18n();
  const methods = node.auth;
  const canSignUp = !!methods?.localSignUp;
  const canSignIn = !!methods?.localSignIn;
  const linked = !!methods?.linkedSignIn;
  const sso = !!methods?.ssoSignIn;
  const others = linked || sso || (methods?.providers.length ?? 0) > 0;
  const [tab, setTab] = useState(canSignIn ? "sign-in" : "sign-up");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [shake, setShake] = useState(0);
  const [ticket, setTicket] = useState<string | null>(null);
  const inAction = useAction(signIn);
  const upAction = useAction(signUp);
  const action = tab === "sign-in" ? inAction : upAction;

  function finish(key: string) {
    if (onDone) onDone(key);
    else navigate({ to: "/$instance", params: { instance: key } });
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (tab === "sign-in") {
      const res = await inAction.go(url, username.trim().toLowerCase(), password);
      if (!res) return setShake((n) => n + 1);
      if (res.ticket) {
        setPassword("");
        return setTicket(res.ticket);
      }
      return finish(res.key!);
    }
    const key = await upAction.go(url, username.trim().toLowerCase(), password, displayName.trim());
    if (!key) return setShake((n) => n + 1);
    finish(key);
  }

  if (ticket) {
    return (
      <TwoFactorStep
        url={url}
        ticket={ticket}
        onBack={(problem) => {
          setTicket(null);
          inAction.setError(problem ?? null);
        }}
        onDone={finish}
      />
    );
  }

  if (!canSignIn && !canSignUp) {
    return (
      <div className="flex flex-col gap-4">
        <Header url={url} node={node} onBack={onBack} />
        {others ? (
          <ProviderButtons url={url} node={node} returnTo={returnTo} />
        ) : (
          <p className="rounded-2xl bg-muted p-4 text-sm text-muted-foreground">
            {t("connect.account.noSignIns")}
          </p>
        )}
      </div>
    );
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-4">
      <Header url={url} node={node} onBack={onBack} />
      {others && (
        <>
          <ProviderButtons url={url} node={node} returnTo={returnTo} />
          <div className="flex items-center gap-3 text-xs font-bold text-muted-foreground uppercase">
            <span className="h-px flex-1 bg-border" /> {t("connect.account.orPassword")} <span className="h-px flex-1 bg-border" />
          </div>
        </>
      )}
      <Tabs value={tab} onValueChange={setTab}>
        <TabsList className="w-full">
          <TabsTrigger value="sign-in" disabled={!canSignIn}>
            {t("connect.account.signIn")}
          </TabsTrigger>
          <TabsTrigger value="sign-up" disabled={!canSignUp}>
            {t("connect.account.signUp")}
          </TabsTrigger>
        </TabsList>
        <TabsContents>
          <TabsContent value="sign-in">
            {!canSignUp && (
              <motion.p
                initial={{ opacity: 0, y: -4 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ duration: 0.35, ease: EASE }}
                className="pt-2 text-xs text-muted-foreground"
              >
                {t("connect.account.noSignUps")}
              </motion.p>
            )}
          </TabsContent>
          <TabsContent value="sign-up">
            <div className="flex flex-col gap-2 pt-2">
              <Label htmlFor="display-name" className="font-bold">
                {t("connect.account.displayName")}
              </Label>
              <Input
                id="display-name"
                placeholder={t("connect.account.displayNameHint")}
                value={displayName}
                onChange={(e) => setDisplayName(e.target.value)}
                maxLength={64}
                className="h-11 rounded-xl"
              />
            </div>
          </TabsContent>
        </TabsContents>
      </Tabs>
      <Credentials
        key={`u${shake}`}
        shake={shake}
        signingIn={tab === "sign-in"}
        username={username}
        onUsername={setUsername}
        password={password}
        onPassword={setPassword}
      />
      <AnimatePresence mode="popLayout">
        {action.error && (
          <motion.p
            {...SLIDE_IN}
            transition={SPRING}
            className="text-sm text-destructive first-letter:uppercase"
          >
            {action.error}
          </motion.p>
        )}
      </AnimatePresence>
      <MotionButton layout="position" transition={SPRING} type="submit" size="lg" disabled={action.pending} className="btn h-11 rounded-xl font-bold">
        {action.pending ? <LoaderCircleIcon className="animate-spin" /> : null}
        {tab === "sign-in" ? t("connect.account.signIn") : t("connect.account.signUp")}
      </MotionButton>
    </form>
  );
}

/** The other ways to sign in this instance offers: single sign-on, then waifu.dev. */
function ProviderButtons({ url, node, returnTo }: { url: string; node: Node; returnTo?: string }) {
  return (
    <>
      {node.auth?.ssoSignIn && <SsoButton url={url} node={node} returnTo={returnTo} />}
      {node.auth?.linkedSignIn && <LinkedButton url={url} node={node} returnTo={returnTo} />}
      <SocialButtons url={url} node={node} returnTo={returnTo} />
    </>
  );
}

/** Username and password; a new key shakes them after a failed try. */
function Credentials({
  shake,
  signingIn,
  username,
  onUsername,
  password,
  onPassword,
}: {
  shake: number;
  signingIn: boolean;
  username: string;
  onUsername: (value: string) => void;
  password: string;
  onPassword: (value: string) => void;
}) {
  const { t } = useI18n();
  return (
    <div className={cn("flex flex-col gap-2", shake > 0 && "shake")}>
      <Label htmlFor="username" className="font-bold">
        {t("connect.account.username")}
      </Label>
      <Input
        id="username"
        autoFocus
        autoComplete="username"
        autoCapitalize="none"
        spellCheck={false}
        placeholder={t("connect.account.usernameHint")}
        value={username}
        onChange={(e) => onUsername(e.target.value)}
        className="h-11 rounded-xl"
        required
      />
      <Label htmlFor="password" className="mt-2 font-bold">
        {t("connect.account.password")}
      </Label>
      <Input
        id="password"
        type="password"
        autoComplete={signingIn ? "current-password" : "new-password"}
        placeholder={signingIn ? "" : t("connect.account.passwordHint")}
        value={password}
        onChange={(e) => onPassword(e.target.value)}
        className="h-11 rounded-xl"
        required
      />
    </div>
  );
}

/**
 * The second step of signing in to an account with two-step sign-in: six
 * digits from the authenticator app, or one of the backup codes.
 */
export function TwoFactorStep({
  url,
  ticket,
  onBack,
  onDone,
}: {
  url: string;
  ticket: string;
  onBack: (problem?: string) => void;
  onDone: (key: string) => void;
}) {
  const [backup, setBackup] = useState(false);
  const [code, setCode] = useState("");
  const [shake, setShake] = useState(0);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { t } = useI18n();

  async function send(value: string) {
    setPending(true);
    setError(null);
    try {
      onDone(await run(verifyTwoFactor(url, ticket, value)));
    } catch (err) {
      const problem = (err as FuwaError).message;
      // A sign-in that ran out goes back to the password.
      if (/ran out/.test(problem)) return onBack(problem);
      setError(problem);
      setShake((n) => n + 1);
    } finally {
      setPending(false);
    }
  }

  return (
    <motion.form
      initial={{ opacity: 0, x: 24 }}
      animate={{ opacity: 1, x: 0 }}
      transition={{ duration: 0.3, ease: EASE }}
      onSubmit={(e) => {
        e.preventDefault();
        if (code.trim()) void send(code);
      }}
      className="flex flex-col gap-4"
    >
      <div className="flex items-center gap-3">
        <button
          type="button"
          onClick={() => onBack()}
          className="grid size-9 place-items-center rounded-full text-muted-foreground transition hover:-translate-x-0.5 hover:bg-muted hover:text-foreground"
          aria-label={t("connect.twoStep.back")}
        >
          <ArrowLeftIcon className="size-4" />
        </button>
        <motion.span
          initial={{ scale: 0, rotate: -30 }}
          animate={{ scale: 1, rotate: 0 }}
          transition={{ type: "spring", stiffness: 500, damping: 16, delay: 0.1 }}
          className="grid size-10 place-items-center rounded-2xl bg-primary/15 text-primary"
        >
          <ShieldCheckIcon className="size-5" />
        </motion.span>
        <div className="min-w-0">
          <p className="font-extrabold">{t("connect.twoStep.title")}</p>
          <p className="text-xs text-muted-foreground">{backup ? t("connect.twoStep.backupHint") : t("connect.twoStep.appHint")}</p>
        </div>
      </div>
      <AnimatePresence mode="wait" initial={false}>
        {backup ? (
          <motion.div key="backup" initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -8 }} transition={{ duration: 0.2, ease: EASE }}>
            <div key={shake} className={cn("flex flex-col gap-2", shake > 0 && "shake")}>
              <Label htmlFor="backup-code" className="font-bold">
                {t("connect.twoStep.backupCode")}
              </Label>
              <Input
                id="backup-code"
                autoFocus
                autoComplete="off"
                autoCapitalize="none"
                spellCheck={false}
                placeholder="abcd-efgh"
                value={code}
                onChange={(e) => setCode(e.target.value)}
                className="h-11 rounded-xl font-mono tracking-widest"
              />
            </div>
          </motion.div>
        ) : (
          <motion.div
            key="app"
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -8 }}
            transition={{ duration: 0.2, ease: EASE }}
            className="flex justify-center"
          >
            <CodeInput id="sign-in-code" label={t("connect.twoStep.code")} onComplete={(value) => void send(value)} disabled={pending} shake={shake} />
          </motion.div>
        )}
      </AnimatePresence>
      <AnimatePresence mode="popLayout">
        {error && (
          <motion.p
            {...SLIDE_IN}
            transition={SPRING}
            className="text-center text-sm text-destructive first-letter:uppercase"
          >
            {error}
          </motion.p>
        )}
      </AnimatePresence>
      {backup && (
        <MotionButton layout="position" transition={SPRING} type="submit" size="lg" disabled={pending || !code.trim()} className="btn h-11 rounded-xl font-bold">
          {pending ? <LoaderCircleIcon className="animate-spin" /> : <KeyRoundIcon />}
          {t("connect.account.signIn")}
        </MotionButton>
      )}
      <motion.button
        layout="position"
        transition={SPRING}
        type="button"
        onClick={() => {
          setBackup((b) => !b);
          setCode("");
          setError(null);
        }}
        className="self-center text-sm font-bold text-primary hover:underline"
      >
        {backup ? t("connect.twoStep.useApp") : t("connect.twoStep.useBackup")}
      </motion.button>
    </motion.form>
  );
}

/**
 * "Continue with waifu.dev": signs in with a linked account, making one here
 * for someone new while the instance takes them. The browser leaves for
 * waifu.dev and comes back to /auth/waifu/callback.
 */
function LinkedButton({ url, node, returnTo }: { url: string; node: Node; returnTo?: string }) {
  const start = useAction(startLinkedSignIn);
  const issuer = issuerName(node.auth?.linkedIssuer);
  return (
    <ProviderButton
      name={issuer}
      icon={<Flower2Icon className="size-5 transition-transform duration-500 group-hover:rotate-[72deg] group-hover:scale-110" />}
      onGo={() => start.go(url, returnTo ?? null)}
      error={start.error}
      closed={!node.auth?.linkedSignUp}
    />
  );
}

/**
 * "Continue with Acme": single sign-on through the identity provider this
 * instance's admins set up, making an account here for someone new while
 * the instance takes them. Comes back to /auth/sso/done.
 */
function SsoButton({ url, node, returnTo }: { url: string; node: Node; returnTo?: string }) {
  const start = useAction(startSsoSignIn);
  const { t } = useI18n();
  const host = node.auth?.ssoHost;
  return (
    <div className="flex flex-col gap-1.5">
      <ProviderButton
        name={node.auth?.ssoName || t("connect.provider.yourOrganization")}
        icon={<BuildingIcon className="size-5 transition-transform duration-500 group-hover:-translate-y-0.5 group-hover:scale-110" />}
        onGo={() => start.go(url, returnTo ?? null)}
        error={start.error}
        closed={!node.auth?.ssoSignUp}
        testId="sso-sign-in"
      />
      {host && <HostLine host={host} testId="sso-sign-in-host" />}
    </div>
  );
}

/** Which site sees your address when a button sends you there. */
function HostLine({ host, testId }: { host: string; testId?: string }) {
  return (
    <motion.p
      initial={{ opacity: 0, y: 4 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: "spring", stiffness: 300, damping: 26, delay: 0.1 }}
      className="text-center text-xs text-balance text-muted-foreground"
      data-testid={testId}
    >
      <GlobeIcon className="mr-1 inline size-3.5 -translate-y-px align-middle" />
      <T k="connect.provider.signsYouInAt" values={{ host: <b className="text-foreground">{host}</b> }} />
    </motion.p>
  );
}

/**
 * "Continue with Google / X / Twitch", for each the instance's admins turned
 * on, each naming the site it sends you to. Comes back to /auth/provider/done.
 */
function SocialButtons({ url, node, returnTo }: { url: string; node: Node; returnTo?: string }) {
  const providers = node.auth?.providers ?? [];
  if (!providers.length) return null;
  return (
    <div className="flex flex-col gap-2.5" data-testid="provider-sign-ins">
      {providers.map((provider, n) => (
        <motion.div
          key={provider.id}
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ type: "spring", stiffness: 320, damping: 28, delay: n * 0.05 }}
        >
          <ProviderSignIn url={url} id={provider.id} name={provider.name} host={provider.host} closed={!provider.signUp} returnTo={returnTo} />
        </motion.div>
      ))}
    </div>
  );
}

function ProviderSignIn({
  url,
  id,
  name,
  host,
  closed,
  returnTo,
}: {
  url: string;
  id: string;
  name: string;
  host: string;
  closed: boolean;
  returnTo?: string;
}) {
  const start = useAction(startProviderSignIn);
  return (
    <div className="flex flex-col gap-1.5">
      <ProviderButton
        name={name}
        icon={<ProviderMark id={id} className="size-[18px] transition-transform duration-500 group-hover:scale-110" />}
        onGo={() => start.go(url, id, returnTo ?? null)}
        error={start.error}
        closed={closed}
        testId={`provider-sign-in-${id}`}
      />
      {host && <HostLine host={host} />}
    </div>
  );
}

/** One "Continue with ..." button: a shine on hover, the icon spinning while the browser leaves. */
export function ProviderButton({
  name,
  icon,
  onGo,
  error,
  closed,
  testId,
  label,
}: {
  name: string;
  icon: ReactNode;
  onGo: () => Promise<unknown>;
  error: string | null;
  closed?: boolean;
  testId?: string;
  label?: string;
}) {
  const [leaving, setLeaving] = useState(false);
  const { t } = useI18n();

  async function go() {
    setLeaving(true);
    // On success the browser is already on its way out.
    if (!(await onGo())) setLeaving(false);
  }

  return (
    <div className="flex flex-col gap-2">
      <motion.button
        type="button"
        onClick={go}
        disabled={leaving}
        data-testid={testId}
        whileHover={{ y: -2 }}
        whileTap={{ scale: 0.97 }}
        transition={{ type: "spring", stiffness: 500, damping: 26 }}
        className="group relative isolate flex h-12 items-center justify-center gap-2.5 overflow-hidden rounded-xl bg-foreground px-4 font-extrabold text-background shadow-[0_14px_30px_-16px_var(--primary)] disabled:cursor-progress"
      >
        <span
          aria-hidden
          className="absolute inset-y-0 -left-1/3 -z-10 w-1/3 -skew-x-12 bg-gradient-to-r from-transparent via-[color-mix(in_srgb,var(--primary)_55%,transparent)] to-transparent opacity-0 transition-[left,opacity] duration-700 group-hover:left-[110%] group-hover:opacity-100"
        />
        <motion.span
          animate={leaving ? { rotate: 360 } : { rotate: 0 }}
          transition={leaving ? { repeat: Infinity, duration: 1.2, ease: "linear" } : { type: "spring", stiffness: 300, damping: 18 }}
          className="grid place-items-center text-primary"
        >
          {icon}
        </motion.span>
        <AnimatePresence mode="wait" initial={false}>
          <motion.span
            key={leaving ? "leaving" : "idle"}
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -8 }}
            transition={{ duration: 0.18 }}
            className="min-w-0 truncate"
          >
            {leaving ? t("connect.provider.leaving", { name }) : (label ?? t("connect.provider.continueWith", { name }))}
          </motion.span>
        </AnimatePresence>
        <ArrowRightIcon className={cn("size-4 transition group-hover:translate-x-1", leaving && "translate-x-2 opacity-0")} />
      </motion.button>
      <AnimatePresence mode="popLayout">
        {error ? (
          <motion.p
            {...SLIDE_IN}
            transition={SPRING}
            className="text-center text-sm text-destructive first-letter:uppercase"
          >
            {error}
          </motion.p>
        ) : closed ? (
          <p className="text-center text-xs text-muted-foreground">
            {t("connect.provider.closed", { name })}
          </p>
        ) : null}
      </AnimatePresence>
    </div>
  );
}

function Header({ url, node, onBack }: { url: string; node: Node; onBack?: () => void }) {
  const { t } = useI18n();
  return (
    <div className="flex items-center gap-3">
      {onBack && (
        <button
          type="button"
          onClick={onBack}
          className="grid size-9 place-items-center rounded-full text-muted-foreground transition hover:-translate-x-0.5 hover:bg-muted hover:text-foreground"
          aria-label={t("connect.account.back")}
        >
          <ArrowLeftIcon className="size-4" />
        </button>
      )}
      <div className="min-w-0">
        <p className="flex min-w-0 items-center gap-2">
          <span className="truncate font-extrabold">{node.name}</span>
          <HostedBadge url={url} />
        </p>
        <p className="truncate text-xs text-muted-foreground">
          <Private text={instanceKey(url)} /> · <BuildLabel node={node} />
        </p>
      </div>
    </div>
  );
}
