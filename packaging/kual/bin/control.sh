#!/bin/sh
# KUAL management entry point. A nonempty root is used only by isolated host tests.
set -u
ROOT=${KINDLE_BATTERY_ROOT:-}
EXT="$ROOT/mnt/us/extensions/kindle-battery-monitor"
JOB="$ROOT/etc/upstart/kindle-battery-monitor.conf"
NAME=kindle-battery-monitor
failure_reported=no
MODEL=AJRLVDTOPT1LE
FIRMWARE='System Software Version: 042-juno_1605_moonshine_rex-414285'
fail() {
    printf '%s\n' "ERROR: $*" >&2
    if command -v report >/dev/null 2>&1; then report "ERROR: $*"; failure_reported=yes; fi
    exit 1
}
[ -z "$ROOT" ] || [ "$ROOT" != / ] || fail 'invalid test root'
actual_model=$(tr -d '\000' < "$ROOT/proc/idme/device_type_id" 2>/dev/null) || actual_model=unknown
actual_firmware=$(sed -n '1p' "$ROOT/etc/version.txt" 2>/dev/null) || actual_firmware=unknown
[ "$actual_model" = "$MODEL" ] && [ "$actual_firmware" = "$FIRMWARE" ] || fail 'unsupported Kindle or firmware'
for service in powerd btfd dbus; do
    [ -f "$ROOT/etc/upstart/$service.conf" ] || fail "missing stock service: $service"
done
for program in bsa_server btfd; do
    [ -x "$ROOT/usr/bin/$program" ] || fail "missing stock Bluetooth program: $program"
done
report() {
    printf '%s\n' "$*"
    # Two bounded lines on the qualified 1072x1448 surface, after KUAL action dispatch.
    if [ -z "$ROOT" ]; then
        row=32
        while [ "$row" -le 39 ]; do
            eips 1 "$row" '                                                              ' >/dev/null 2>&1 || :
            row=$((row + 1))
        done
        row=32
        printf '%s\n' "$*" | sed 's/[^A-Za-z0-9 .%_-]/ /g' | fold -s -w 60 | head -8 | while IFS= read -r line; do
            eips 1 "$row" "$line" >/dev/null 2>&1 || :
            row=$((row + 1))
        done
    fi
}
status() {
    if [ ! -f "$JOB" ]; then report 'Not installed'; return 0; fi
    service=$(timeout 55 initctl status "$NAME" 2>&1) || { report "Status unavailable: $service"; return 1; }
    boot=disabled
    grep -q '^start on ' "$JOB" && boot=enabled
    version=$(sed -n '1p' "$EXT/state/version" 2>/dev/null) || version=legacy
    battery=$(timeout 3 lipc-get-prop com.lab126.powerd battLevel 2>/dev/null) || battery=unavailable
    case "$battery" in ''|*[!0-9]*) battery=unavailable ;; *) [ "$battery" -le 100 ] || battery=unavailable ;; esac
    failure=$(tail -80 "$ROOT/var/log/kindle-battery-monitor.jsonl" 2>/dev/null | grep 'monitor_failed' | tail -1 | sed 's/.*"error":"//;s/".*$//') || failure=
    summary="Version $version; boot $boot; $service; battery $battery"
    [ -z "$failure" ] || summary="$summary. Last failure: $failure"
    report "$summary"
}
PAYLOAD="$EXT/payload"
STATE="$EXT/state"
BIN="$EXT/bin/kindle-battery-monitor"
LOCK="$ROOT/var/tmp/kindle-battery-control.lock"
root_mode=
root_changed=no
locked=no
transaction_active=no
value() { sed -n "s/^$1=//p" "$PAYLOAD/manifest"; }
sha() { sha256sum "$1" | cut -d ' ' -f1; }
safe_paths() {
    for path in "$ROOT/mnt/us" "$ROOT/mnt/us/extensions" "$ROOT/etc/upstart" "$EXT" "$EXT/bin" "$PAYLOAD" "$STATE" "$TX" "$JOB" "$JOB.new" "$BIN" "$BIN.new" "$LOCK" "$PAYLOAD/manifest" "$PAYLOAD/monitor" "$PAYLOAD/automatic.conf" "$PAYLOAD/on-demand.conf" "$STATE/version" "$STATE/job.sha256" "$STATE/binary.sha256" "$STATE/boot-job.backup" "$STATE/uninstall-job.backup" "$ROOT/var/log/kindle-battery-monitor.jsonl"; do
        [ ! -L "$path" ] || fail "symlink destination refused: $path"
    done
}
restore_root() {
    if [ "$root_changed" = yes ]; then
        mntroot "$root_mode" >/dev/null 2>&1 || return 1
        root_changed=no
    fi
}
finish() {
    code=$?
    trap - EXIT HUP INT TERM
    if [ "$code" -ne 0 ] && [ "$transaction_active" = yes ]; then
        if recover_transaction; then
            printf '%s\n' 'Previous installation recovered.' >&2
        else
            printf '%s\n' 'ERROR: recovery failed; service left stopped where possible; recovery record retained.' >&2
        fi
    fi
    if ! restore_root; then printf '%s\n' 'ERROR: root mount restoration failed' >&2; code=1; fi
    [ "$locked" = no ] || rm -f "$LOCK"
    [ "${KBM_SELF_COPY:-}" != "$0" ] || rm -f "$0"
    if [ "$code" -ne 0 ] && [ "$failure_reported" = no ]; then report 'Action failed. Inspect the installation before retrying.'; fi
    exit "$code"
}
writable_root() {
    [ "$root_changed" != yes ] || return 0
    root_mode=$(awk '$2 == "/" {n=split($4,a,","); for(i=1;i<=n;i++) if(a[i]=="ro" || a[i]=="rw") print a[i]}' "$ROOT/proc/mounts")
    case "$root_mode" in
        ro) mntroot rw >/dev/null 2>&1 || fail 'cannot make root writable'; root_changed=yes ;;
        rw) ;;
        *) fail 'cannot establish original root mount mode' ;;
    esac
}
validate_payload() {
    [ "$(value format)" = 1 ] || fail 'invalid release manifest'
    [ "$(value device_type)" = "$MODEL" ] && [ "$(value firmware)" = "$FIRMWARE" ] || fail 'release targets a different Kindle or firmware'
    for entry in monitor automatic.conf on-demand.conf; do [ -s "$PAYLOAD/$entry" ] || fail "missing release file: $entry"; done
    for field in binary_sha256 automatic_sha256 on_demand_sha256 controller_sha256 legacy_binary_sha256 legacy_job_sha256; do
        expected=$(value "$field")
        [ "${#expected}" -eq 64 ] || fail "invalid manifest hash: $field"
        case "$expected" in *[!0-9a-f]*) fail "invalid manifest hash: $field" ;; esac
    done
    [ "$(sha "$0")" = "$(value controller_sha256)" ] || fail 'controller checksum mismatch' 
    release=$(value version)
    case "$release" in ''|*[!0-9A-Za-z._-]*) fail 'invalid release version' ;; esac
    [ "$(sha "$PAYLOAD/monitor")" = "$(value binary_sha256)" ] || fail 'payload checksum mismatch'
    [ "$(sha "$PAYLOAD/automatic.conf")" = "$(value automatic_sha256)" ] || fail 'automatic job checksum mismatch'
    [ "$(sha "$PAYLOAD/on-demand.conf")" = "$(value on_demand_sha256)" ] || fail 'on-demand job checksum mismatch'
    [ "$(od -An -tx1 -N7 "$PAYLOAD/monitor" | tr -d ' \n')" = 7f454c46010101 ] || fail 'payload is not a 32-bit little-endian ELF'
    [ "$(od -An -tx1 -j18 -N2 "$PAYLOAD/monitor" | tr -d ' \n')" = 2800 ] || fail 'payload is not ARM'
}
record_installation() {
    mkdir -p "$STATE" || fail 'cannot create installation state'
    printf '%s\n' "$release" > "$STATE/version" && sha "$BIN" > "$STATE/binary.sha256" && sha "$JOB" > "$STATE/job.sha256"
}
service_state() {
    service=$(timeout 55 initctl status "$NAME" 2>&1) || fail "service state unavailable: $service"
    case "$service" in
        *'start/running'*) running=yes ;;
        *'stop/waiting'*) running=no ;;
        *) fail "service is transitioning: $service" ;;
    esac
}
owned_installation() {
    [ -f "$JOB" ] && [ -f "$BIN" ] || fail 'monitor is not installed'
    expected_job=$(cat "$STATE/job.sha256" 2>/dev/null) || expected_job=$(value legacy_job_sha256)
    expected_binary=$(cat "$STATE/binary.sha256" 2>/dev/null) || expected_binary=$(value legacy_binary_sha256)
    [ "$(sha "$JOB")" = "$expected_job" ] || fail 'unknown or modified startup job'
    [ "$(sha "$BIN")" = "$expected_binary" ] || fail 'unknown or modified installed runtime'
}
set_session() {
    service_state
    if [ "$running" != "$1" ]; then
        case "$1" in
            yes) timeout 55 initctl start "$NAME" || fail 'monitor start failed' ;;
            no)
                previous_shutdown=$(tail -80 "$ROOT/var/log/kindle-battery-monitor.jsonl" 2>/dev/null | grep -E 'monitor_stopped|monitor_failed' | tail -1) || previous_shutdown=
                timeout 55 initctl stop "$NAME" || fail 'monitor stop failed; installation preserved'
                shutdown=$(tail -80 "$ROOT/var/log/kindle-battery-monitor.jsonl" 2>/dev/null | grep -E 'monitor_stopped|monitor_failed' | tail -1) || shutdown=
                case "$shutdown" in *monitor_stopped*) ;; *) fail 'monitor cleanup is uncertain; installation preserved' ;; esac
                [ "$shutdown" != "$previous_shutdown" ] || fail 'no fresh clean shutdown receipt; installation preserved'
                ;;
        esac
        service_state
        [ "$running" = "$1" ] || fail 'service did not reach requested state'
    fi
}
set_boot() {
    current=disabled
    grep -q '^start on ' "$JOB" && current=enabled
    if [ "$current" != "$1" ]; then
        writable_root
        template=on-demand.conf
        [ "$1" != enabled ] || template=automatic.conf
        mkdir -p "$STATE" && cp "$JOB" "$STATE/boot-job.backup" || fail 'cannot back up boot setting'
        cp "$PAYLOAD/$template" "$JOB.new" && mv -f "$JOB.new" "$JOB" || fail 'startup setting replacement failed'
        if ! timeout 10 initctl reload-configuration; then
            cp "$STATE/boot-job.backup" "$JOB.new" && mv -f "$JOB.new" "$JOB" && timeout 10 initctl reload-configuration || printf '%s\n' 'ERROR: boot-setting recovery failed; backup retained' >&2
            fail 'startup setting reload failed'
        fi
        mkdir -p "$STATE" && sha "$JOB" > "$STATE/job.sha256" || fail 'startup ownership state failed'
        rm -f "$STATE/boot-job.backup"
    fi
}
manage() {
    owned_installation
    case "$1" in
        enable) validate_payload; set_boot enabled; set_session yes ;;
        disable) validate_payload; set_session no; set_boot disabled ;;
        start) set_session yes ;;
        stop) set_session no ;;
    esac
    restore_root || fail 'root mount restoration failed'
    report "$1 complete"
}
uninstall() {
    [ ! -e "$TX" ] || fail 'unfinished transaction; recover before uninstall'
    if [ -e "$JOB" ] || [ -e "$BIN" ]; then
        owned_installation
        set_session no
        writable_root
        cp "$JOB" "$STATE/uninstall-job.backup" || fail 'cannot save uninstall recovery job'
        rm -f "$JOB" || fail 'cannot remove owned startup job'
        if ! timeout 10 initctl reload-configuration; then
            cp "$STATE/uninstall-job.backup" "$JOB" && timeout 10 initctl reload-configuration || printf '%s\n' 'ERROR: uninstall job recovery failed' >&2
            fail 'uninstall reload failed; extension retained'
        fi
    else
        validate_payload
    fi
    rm -rf "$EXT" || fail 'extension removal incomplete'
    rm -f "$ROOT/var/log/kindle-battery-monitor.jsonl" || fail 'monitor log removal failed'
    restore_root || fail 'root mount restoration failed'
    report 'Uninstalled entire extension and startup job'
}
TX="$STATE/transaction"
recover_transaction() {
    [ -f "$TX/ready" ] || return 1
    for name in ready had-install running monitor job version binary.sha256 job.sha256; do
        [ ! -L "$TX/$name" ] || return 1
    done
    if [ -f "$JOB" ]; then
        current_job=$(sha "$JOB")
        previous_job=$(cat "$TX/job.sha256" 2>/dev/null) || previous_job=none
        [ "$current_job" = "$previous_job" ] || [ "$current_job" = "$(value automatic_sha256)" ] || [ "$current_job" = "$(value on_demand_sha256)" ] || return 1
    fi
    observed=$(timeout 55 initctl status "$NAME" 2>&1) || observed=absent
    case "$observed" in *'start/running'*) timeout 55 initctl stop "$NAME" || return 1 ;; esac
    writable_root
    previous=$(cat "$TX/had-install") || return 1
    case "$previous" in
        yes)
            [ "$(sha "$TX/monitor")" = "$(cat "$TX/binary.sha256")" ] || return 1
            [ "$(sha "$TX/job")" = "$(cat "$TX/job.sha256")" ] || return 1
            cp "$TX/monitor" "$BIN.new" && chmod 755 "$BIN.new" && mv -f "$BIN.new" "$BIN" || return 1
            cp "$TX/job" "$JOB.new" && mv -f "$JOB.new" "$JOB" || return 1
            cp "$TX/version" "$STATE/version" && cp "$TX/binary.sha256" "$STATE/binary.sha256" && cp "$TX/job.sha256" "$STATE/job.sha256" || return 1
            ;;
        no) rm -f "$JOB" "$BIN" "$STATE/version" "$STATE/binary.sha256" "$STATE/job.sha256" || return 1 ;;
        *) return 1 ;;
    esac
    timeout 10 initctl reload-configuration || return 1
    if [ "$previous" = yes ] && [ "$(cat "$TX/running")" = yes ]; then
        timeout 55 initctl start "$NAME" || return 1
    fi
    rm -rf "$TX" || return 1
    transaction_active=no
}
install() {
    validate_payload
    old_running=yes
    template=automatic.conf
    had_install=no
    terminal=$(tail -80 "$ROOT/var/log/kindle-battery-monitor.jsonl" 2>/dev/null | grep -E 'monitor_stopped|monitor_failed' | tail -1) || terminal=
    case "$terminal" in *monitor_failed*) fail 'previous cleanup failure requires explicit inspection or a clean manual session' ;; esac
    if [ -e "$JOB" ] || [ -e "$BIN" ]; then
        owned_installation
        had_install=yes
        service_state
        old_running=$running
        grep -q '^start on ' "$JOB" || template=on-demand.conf
        set_session no
    fi
    mkdir -p "$STATE" "$EXT/bin" || fail 'cannot create installation directories'
    [ ! -e "$TX" ] || fail 'unfinished transaction; recovery required'
    mkdir "$TX" || fail 'cannot create recovery record'
    printf '%s\n' "$had_install" > "$TX/had-install"
    printf '%s\n' "$old_running" > "$TX/running"
    if [ "$had_install" = yes ]; then
        cp "$BIN" "$TX/monitor" && cp "$JOB" "$TX/job" || fail 'cannot back up prior installation'
        sha "$BIN" > "$TX/binary.sha256"
        sha "$JOB" > "$TX/job.sha256"
        if [ -f "$STATE/version" ]; then cp "$STATE/version" "$TX/version"; else printf '%s\n' legacy > "$TX/version"; fi
    fi
    printf '%s\n' ready > "$TX/ready"
    transaction_active=yes
    writable_root
    cp "$PAYLOAD/monitor" "$BIN.new" && chmod 755 "$BIN.new" && mv -f "$BIN.new" "$BIN" || fail 'runtime replacement failed'
    cp "$PAYLOAD/$template" "$JOB.new" && mv -f "$JOB.new" "$JOB" || fail 'job replacement failed'
    timeout 10 initctl reload-configuration || fail 'job reload failed'
    record_installation || fail 'installation ownership record failed'
    set_session "$old_running"
    restore_root || fail 'root mount restoration failed'
    rm -rf "$TX" || fail 'cannot remove recovery record'
    transaction_active=no
    report "Installed $release; prior session and boot setting preserved"
}
safe_paths
case "${1:-}" in
    status) status; exit $? ;;
    cancel) report 'Uninstall cancelled'; exit 0 ;;
    uninstall) fail 'uninstall requires confirmation' ;;
    install|start|stop|enable|disable|uninstall-confirm) ;;
    *) fail 'unknown management action' ;;
esac
if [ "$1" = uninstall-confirm ] && [ "${KBM_SELF_COPY:-}" != "$0" ]; then
    self_copy=$(mktemp "$ROOT/var/tmp/kbm-uninstall.XXXXXX") || fail 'cannot stage uninstall controller'
    cp "$0" "$self_copy" || fail 'cannot copy uninstall controller'
    KBM_SELF_COPY="$self_copy"; export KBM_SELF_COPY
    exec sh "$self_copy" "$1"
fi
exec 9> "$LOCK" || fail 'cannot open management lock'
flock -n 9 || fail 'another management action is busy'
locked=yes
trap finish EXIT
trap 'exit 130' HUP INT TERM
if [ -e "$TX" ]; then
    [ "$1" = install ] || fail 'unfinished transaction; use Install/update to recover first'
    if recover_transaction; then
        restore_root || fail 'recovery root mount restoration failed'
        report 'Recovered previous installation. Run Install/update again to update.'
        exit 0
    fi
    fail 'unfinished transaction recovery failed; inspect retained recovery files'
fi
case "$1" in
    install) install ;;
    start|stop|enable|disable) manage "$1" ;;
    uninstall-confirm) uninstall ;;
esac
