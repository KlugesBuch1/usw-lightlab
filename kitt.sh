#!/bin/sh

COLS="1/2 3/4 5/6 7/8 9/10 11/12 13/14 15/16 17/18 19/20 21/22 23/24"
COLS="$COLS 25/26 27/28 29/30 31/32 33/34 35/36 37/38 39/40 41/42 43/44 45/46 47/48"

MS=22
DWELL=1
SYNC=auto

MODE=/proc/led/led_mode
CODE=/proc/led/led_code
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

fmt=
i=0
while [ "$i" -lt "$n" ]; do
    fmt="$fmt%s %02x 00 00 100\n"
    i=$((i + 1))
    eval "F$i=\"\$fmt\""
done

CHUNK=$n
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

write_chunk() {
    if ( printf "$f" $ARGS 2>/dev/null > "$CODE" ); then
        fail=0
    else
        fail=$((fail + 1))
        if [ "$fail" -ge 20 ] && [ "$in_cleanup" != 1 ]; then
            die "$CODE rejects the frames - use a smaller CHUNK or SYNC=color"
        fi
    fi
}

put() {
    if [ "$use_code" != 1 ]; then
        if printf '%s r %s\n' "$1" "$(( $2 * 257 ))" 2>/dev/null > "$COLOR"; then
            fail=0
        else
            fail=$((fail + 1))
            if [ "$fail" -ge 40 ] && [ "$in_cleanup" != 1 ]; then
                die "$COLOR rejects the writes"
            fi
        fi
        return
    fi
    ARGS="$ARGS $1 $2"
    cnt=$((cnt + 1))
    if [ "$cnt" -ge "$CHUNK" ]; then
        eval "f=\$F$cnt"
        write_chunk
        ARGS=
        cnt=0
    fi
}

flush_chunk() {
    if [ "$cnt" -gt 0 ]; then
        eval "f=\$F$cnt"
        write_chunk
        ARGS=
        cnt=0
    fi
}

render() {
    rpos=$1
    rdir=$2
    i=0
    ARGS=
    cnt=0
    for c in $COLS; do
        level "$(( (rpos - i) * rdir ))"
        put "${c%%/*}" "$R"
        case $c in
            */*) put "${c#*/}" "$R" ;;
        esac
        i=$((i + 1))
    done
    flush_chunk
}

clear_all() {
    if [ "$use_code" = 1 ]; then
        ARGS=
        cnt=0
        for p in $PORTS; do
            put "$p" 0
        done
        flush_chunk
    else
        for p in $PORTS; do
            printf '%s r 0\n' "$p" 2>/dev/null > "$COLOR"
            printf '%s g 0\n' "$p" 2>/dev/null > "$COLOR"
            printf '%s b 0\n' "$p" 2>/dev/null > "$COLOR"
        done
    fi
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

echo 0 > "$MODE" 2>/dev/null || die "cannot set led_mode to 0 (not root?)"

use_code=0
if [ "$SYNC" != color ] && [ -w "$CODE" ]; then
    best=0
    for k in 1 2 4 8 12 16 20 24; do
        [ "$k" -le "$n" ] || break
        eval "f=\$F$k"
        args=
        j=0
        for pp in $PORTS; do
            [ "$j" -lt "$k" ] || break
            args="$args $pp 0"
            j=$((j + 1))
        done
        if ( printf "$f" $args 2>/dev/null > "$CODE" ); then
            best=$k
        else
            break
        fi
    done
    if [ "$best" -gt 0 ]; then
        use_code=1
        CHUNK=$best
    fi
fi

if [ "$SYNC" = code ] && [ "$use_code" != 1 ]; then
    die "led_code does not accept frames - SYNC=code not possible"
fi
if [ "$use_code" != 1 ]; then
    [ -w "$COLOR" ] || die "neither led_code nor led_color is writable"
fi

clear_all

if [ "$use_code" = 1 ]; then
    how="led_code, $CHUNK rows/write"
else
    how="led_color (not synchronized - led_code rejected)"
fi
echo "kitt: $n columns / $np LEDs, ${MS} ms/step, timer=$timer, $how - Ctrl+C to stop."

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
