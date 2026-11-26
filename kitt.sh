#!/bin/sh

COLS="1/2 3/4 5/6 7/8 9/10 11/12 13/14 15/16 17/18 19/20 21/22 23/24"
COLS="$COLS 25/26 27/28 29/30 31/32 33/34 35/36 37/38 39/40 41/42 43/44 45/46 47/48"

MS=22
DWELL=1

MODE=/proc/led/led_mode
# The firmware rejects led_code frames, so all output goes through led_color.
COLOR=/proc/led/led_color

die() {
    echo "kitt: $*" >&2
    exit 1
}

[ -w "$MODE" ] || die "$MODE missing or not writable (id; ls -l /proc/led)"

uid=$(id -u 2>/dev/null)
if [ -n "$uid" ] && [ "$uid" != 0 ]; then
    die "not root (uid=$uid) - /proc/led requires root."
fi

secs=$(printf '0.%03d' "$MS")

nowcs() {
    read -r u _ < /proc/uptime || { NOW=0; return; }
    case $u in
        *.*) NOW=${u%.*}${u#*.} ;;
        *)   NOW=${u}00 ;;
    esac
}

if sleep "$secs" 2>/dev/null; then
    timer=sleep
    nap() { sleep "$secs"; }
elif /bin/sleep "$secs" 2>/dev/null; then
    timer=/bin/sleep
    nap() { /bin/sleep "$secs"; }
elif usleep $((MS * 1000)) 2>/dev/null; then
    timer=usleep
    nap() { usleep $((MS * 1000)); }
elif /bin/busybox usleep $((MS * 1000)) 2>/dev/null; then
    timer=busybox-usleep
    nap() { /bin/busybox usleep $((MS * 1000)); }
elif [ -r /proc/uptime ]; then
    timer="uptime-poll (CPU-heavy)"
    nap() {
        nowcs
        end=$((NOW + MS / 10 + 1))
        while [ "$NOW" -lt "$end" ]; do nowcs; done
    }
else
    die "no timer: sleep cannot handle '$secs' and usleep is missing.
               Please report: busybox | head -1; command -v usleep"
fi

if [ -r /tmp/kitt.pid ]; then
    old=$(cat /tmp/kitt.pid 2>/dev/null)
    if [ -n "$old" ] && kill -0 "$old" 2>/dev/null; then
        die "already running as PID $old - stop it first: kill $old
               (or, if that is not the script: rm /tmp/kitt.pid)"
    fi
    rm -f /tmp/kitt.pid
fi

n=0
np=0
PORTS=
for c in $COLS; do
    n=$((n + 1))
    PORTS="$PORTS ${c%%/*}"
    np=$((np + 1))
    case $c in
        */*)
            PORTS="$PORTS ${c#*/}"
            np=$((np + 1))
            ;;
    esac
done
if [ "$n" -ne 24 ] || [ "$np" -ne 48 ]; then
    die "layout is not the USW-Pro-Max-48 (expected 24 columns / 48 LEDs, got $n/$np)"
fi

fail=0
in_cleanup=0

level() {
    case $1 in
        0)  R=255 ;;
        -1) R=40 ;;
        1)  R=140 ;;
        2)  R=64 ;;
        3)  R=24 ;;
        4)  R=7 ;;
        *)  R=0 ;;
    esac
}

put() {
    if printf '%s r %s\n' "$1" "$(( $2 * 257 ))" 2>/dev/null > "$COLOR"; then
        fail=0
    else
        fail=$((fail + 1))
        if [ "$fail" -ge 40 ] && [ "$in_cleanup" != 1 ]; then
            die "$COLOR rejects the writes"
        fi
    fi
}

render() {
    rpos=$1
    rdir=$2
    i=0
    for c in $COLS; do
        level "$(( (rpos - i) * rdir ))"
        put "${c%%/*}" "$R"
        case $c in
            */*) put "${c#*/}" "$R" ;;
        esac
        i=$((i + 1))
    done
}

clear_all() {
    for p in $PORTS; do
        printf '%s r 0\n' "$p" 2>/dev/null > "$COLOR"
        printf '%s g 0\n' "$p" 2>/dev/null > "$COLOR"
        printf '%s b 0\n' "$p" 2>/dev/null > "$COLOR"
    done
}

cleanup() {
    in_cleanup=1
    clear_all
    echo 1 > "$MODE" 2>/dev/null
    rm -f /tmp/kitt.pid
    echo "kitt: stopped, LEDs back under firmware control."
}
trap cleanup EXIT
trap 'exit 0' INT TERM

echo $$ > /tmp/kitt.pid

[ -w "$COLOR" ] || die "$COLOR missing or not writable (id; ls -l /proc/led)"

echo 0 > "$MODE" 2>/dev/null || die "cannot set led_mode to 0 (not root?)"

clear_all

echo "kitt: $n columns / $np LEDs, ${MS} ms/step, timer=$timer - Ctrl+C to stop."

last=$((n - 1))
pos=0
dir=1
hold=0

while :; do
    render "$pos" "$dir"
    nap

    if [ "$pos" -eq 0 ] || [ "$pos" -eq "$last" ]; then
        if [ "$hold" -lt "$DWELL" ]; then
            hold=$((hold + 1))
            continue
        fi
        hold=0
        if [ "$pos" -eq "$last" ]; then dir=-1; else dir=1; fi
    fi
    pos=$((pos + dir))
done
