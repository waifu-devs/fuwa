/** The last place you were in the app, on this device, so it opens there next time. */

const LAST_PATH = "fuwa:last-path";

export function rememberedPath(): string | null {
  try {
    return localStorage.getItem(LAST_PATH);
  } catch {
    return null;
  }
}

export function rememberPath(pathname: string) {
  try {
    localStorage.setItem(LAST_PATH, pathname);
  } catch {
    // Not remembered; the app still opens on the first server next time.
  }
}
