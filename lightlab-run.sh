#!/bin/sh

PIDFILE=/tmp/usw-lightlab.pid
MODE=/proc/led/led_mode
COLOR=/proc/led/led_color

child=
watchdog=
finished=0

clear_all() {
    port=1
    while [ "$port" -le 48 ]; do
        printf '%s r 0\n' "$port" > "$COLOR" 2>/dev/null
        printf '%s g 0\n' "$port" > "$COLOR" 2>/dev/null
        printf '%s b 0\n' "$port" > "$COLOR" 2>/dev/null
        port=$((port + 1))
    done
}

cleanup() {
    [ "$finished" = 1 ] && return
    finished=1
    trap - EXIT INT TERM HUP
    if [ -n "$watchdog" ]; then
        kill "$watchdog" 2>/dev/null
    fi
    if [ -n "$child" ]; then
        kill -TERM "$child" 2>/dev/null
        tries=0
        while kill -0 "$child" 2>/dev/null && [ "$tries" -lt 20 ]; do
            sleep 0.1
            tries=$((tries + 1))
        done
        kill -KILL "$child" 2>/dev/null
        wait "$child" 2>/dev/null
    fi
    clear_all
    echo 1 > "$MODE" 2>/dev/null
    current=$(cat "$PIDFILE" 2>/dev/null)
    if [ "$current" = "$$" ]; then
        rm -f "$PIDFILE"
    fi
}

if (set -C; echo "$$" > "$PIDFILE") 2>/dev/null; then
    :
else
    old=$(cat "$PIDFILE" 2>/dev/null)
    case $old in
        ''|*[!0-9]*) old= ;;
    esac
    if [ -n "$old" ] && kill -0 "$old" 2>/dev/null; then
        oldcmd=$(tr '\0' ' ' < "/proc/$old/cmdline" 2>/dev/null)
        case $oldcmd in
            *lightlab-run.sh*)
                oldscript=$(printf '%s\n' "$oldcmd" | awk '{for (i = 1; i <= NF; i++) if ($i ~ /lightlab-run\.sh$/) {print $(i + 1); exit}}')
                oldscript=${oldscript##*/}
                if [ -n "$oldscript" ]; then
                    echo "lightlab-run: already running as PID $old with script $oldscript" >&2
                else
                    echo "lightlab-run: already running as PID $old" >&2
                fi
                exit 1
                ;;
            *)
                echo "lightlab-run: warning: PID $old is alive but is not lightlab-run.sh; unlinking $PIDFILE without signalling" >&2
                rm -f "$PIDFILE"
                ;;
        esac
    else
        rm -f "$PIDFILE"
    fi
    if ! (set -C; echo "$$" > "$PIDFILE") 2>/dev/null; then
        echo "lightlab-run: cannot claim $PIDFILE" >&2
        exit 1
    fi
fi

trap cleanup EXIT INT TERM HUP

script=$1
if [ -z "$script" ]; then
    echo "lightlab-run: no animation script was given" >&2
    exit 1
fi
if [ ! -r "$script" ]; then
    echo "lightlab-run: cannot read $script" >&2
    exit 1
fi

name=${script##*/}
echo "lightlab-run: supervisor $$ started $name, pidfile $PIDFILE" >&2

sh "$script" &
child=$!

if [ "$2" = attached ]; then
    exec 3<&0
    (
        while IFS= read -r _ <&3; do :; done
        kill -TERM "$child" 2>/dev/null
    ) &
    watchdog=$!
    exec 3<&-
fi

wait "$child"
status=$?
echo "lightlab-run: animation exited with status $status" >&2
exit "$status"
