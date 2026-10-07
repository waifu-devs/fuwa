#!/usr/bin/env bash
# The desktop app under Xvfb, signed in as alice on the seeded instance.
#   DISP=78 desk.sh start [BINARY]   (re)starts display :78 (1300x820) and the app (a fresh home each time)
#   DISP=78 desk.sh shot NAME        the 1280x800 window -> $FUWA_VISUAL_DIR/out/NAME-desktop.png
#   DISP=78 desk.sh do click X Y | move X Y | key ctrl+comma | type TEXT | scroll X Y N   (window coordinates)
#   DISP=78 desk.sh stop
# Each person or agent working at once takes their own DISP (default 77).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
vis=${FUWA_VISUAL_DIR:-/tmp/fuwa-visual}
D=${DISP:-77}
export DISPLAY=:$D
mkdir -p "$vis/out"
case "${1:-}" in
start)
  root=$(git rev-parse --show-toplevel 2>/dev/null || (cd "$here/../../.." && pwd))
  bin=${2:-$vis/bin/$(basename "$root")/fuwa-desktop}
  [ -x "$bin" ] || { echo "no binary at $bin: run dcargo build in your checkout first"; exit 1; }
  pkill -f "fuwa-desktop-$D" 2>/dev/null || true
  pgrep -f "Xvfb :$D " >/dev/null || { nohup Xvfb ":$D" -screen 0 1300x820x24 -nolisten tcp >"$vis/xvfb-$D.log" 2>&1 & sleep 1; }
  home=$vis/desk-home-$D; rm -rf "$home"; mkdir -p "$home/config"
  url=$(node -e 'const s=require(process.argv[1]);console.log(s.url)' "$vis/state.json")
  token=$(node -e 'const s=require(process.argv[1]);console.log(s.tokens[process.argv[2]])' "$vis/state.json" "${AS:-alice}")
  echo "[{\"url\":\"$url\",\"token\":\"$token\"}]" > "$home/config/instances.json"
  [ -n "${SETTINGS:-}" ] && cp "$SETTINGS" "$home/config/settings.json"
  # Lavapipe (software Vulkan) where there's no GPU.
  icd=/usr/share/vulkan/icd.d/lvp_icd.json
  [ -z "${VK_ICD_FILENAMES:-}" ] && [ -f $icd ] && export VK_ICD_FILENAMES=$icd
  FUWA_DESKTOP_HOME=$home FUWA_DESKTOP_KEYCHAIN=off RUST_LOG=${RUST_LOG:-warn} \
    nohup -- bash -c 'exec -a "fuwa-desktop-$0" "$1"' "$D" "$bin" > "$vis/desktop-$D.log" 2>&1 &
  sleep "${WAIT:-6}"; "$0" shot _origin >/dev/null; echo started ;;
shot)
  # Find the window on the screen, then capture just it.
  python3 "$here/xshot.py" "$vis/full-$D.png" >/dev/null
  read -r x y < <(python3 "$here/probe.py" "$vis/full-$D.png" window)
  echo "$x $y" > "$vis/origin-$D"
  python3 "$here/xshot.py" "$vis/out/$2-desktop.png" "$x" "$y" 1280 800 >/dev/null; echo "$vis/out/$2-desktop.png" ;;
do) shift; read -r XOFF YOFF < "$vis/origin-$D"; XOFF=$XOFF YOFF=$YOFF python3 "$here/xinput.py" "$@" ;;
stop) pkill -f "fuwa-desktop-$D" || true; pkill -f "Xvfb :$D " || true ;;
*) sed -n 2,8p "$0" ;;
esac
