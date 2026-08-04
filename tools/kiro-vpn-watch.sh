#!/bin/sh

set -eu

SERVICE_LABEL="com.kiro.rs"
POLL_INTERVAL="${KIRO_VPN_POLL_INTERVAL:-3}"
STATE_FILE="${KIRO_VPN_STATE_FILE:-$HOME/Library/Caches/kiro-rs/vpn-state}"

vpn_state() {
    if [ -n "${KIRO_VPN_STATE_OVERRIDE:-}" ]; then
        printf '%s\n' "$KIRO_VPN_STATE_OVERRIDE"
        return
    fi

    if /usr/sbin/scutil --nwi | /usr/bin/grep -Eq 'Network interfaces:.*(^|[[:space:]])utun[0-9]+([[:space:]]|$)'; then
        printf 'active\n'
    else
        printf 'inactive\n'
    fi
}

restart_kiro() {
    domain="gui/$(/usr/bin/id -u)"
    printf '%s VPN connected; restarting %s\n' "$(/bin/date '+%Y-%m-%dT%H:%M:%S%z')" "$SERVICE_LABEL"
    /bin/launchctl kickstart -k "$domain/$SERVICE_LABEL"
}

check_once() {
    current_state="$(vpn_state)"
    previous_state=""

    if [ -f "$STATE_FILE" ]; then
        previous_state="$(/bin/cat "$STATE_FILE")"
    fi

    /bin/mkdir -p "$(/usr/bin/dirname "$STATE_FILE")"
    printf '%s\n' "$current_state" > "$STATE_FILE"

    if [ "$previous_state" = "inactive" ] && [ "$current_state" = "active" ]; then
        restart_kiro
    fi
}

if [ "${1:-}" = "--once" ]; then
    check_once
    exit 0
fi

while true; do
    check_once
    /bin/sleep "$POLL_INTERVAL"
done
