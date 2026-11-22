#!/bin/bash
# Check the exact notarized qualification archive in a disposable macOS guest.
set -euo pipefail

fail() {
    printf '%s\n' "$*" >&2
    exit 1
}

[[ $# == 2 ]] || fail 'Expected the guest archive path and trusted SHA-256.'
archive="$1"
expected_sha="$2"
[[ "$archive" == /* && "$expected_sha" =~ ^[0-9a-f]{64}$ ]] ||
    fail 'Expected an absolute archive path and a lowercase SHA-256.'
case "$(/usr/sbin/sysctl -n hw.model)" in
    VirtualMac*) ;;
    *) fail 'Run this check in a disposable macOS virtual machine.' ;;
esac
version="$(/usr/bin/sw_vers -productVersion)"
printf 'Guest OS: %s\n' "$version"
[[ "${version%%.*}" =~ ^[0-9]+$ && "${version%%.*}" -ge 27 ]] ||
    fail 'Marsa first-launch qualification requires macOS 27 or newer.'

uid="$(/usr/bin/id -u)"
[[ "$uid" != 0 && "$uid" == "$(/usr/bin/stat -f '%u' /dev/console)" ]] ||
    fail 'Run the check as the unprivileged guest account logged into the desktop.'
[[ ! -e "$HOME/.marsa" && ! -L "$HOME/.marsa" ]] ||
    fail 'First launch requires a local account without prior Marsa state.'

route="$(/sbin/route -n get -inet default)" || fail 'Guest IPv4 default route is missing.'
nic="$(/usr/bin/awk '/interface:/{print $2}' <<< "$route")"
router="$(/usr/bin/awk '/gateway:/{print $2}' <<< "$route")"
[[ -n "$nic" && -n "$router" ]] || fail 'Guest IPv4 default route is incomplete.'
addr="$(/usr/sbin/ipconfig getifaddr "$nic")" || fail 'Guest IPv4 address is missing.'
[[ -n "$addr" && "$addr" != 169.254.* ]] || fail 'Guest has no usable IPv4 address.'
printf 'Guest IPv4: assigned\n'
/sbin/ping -n -c 1 -W 800 "$router" >/dev/null || fail 'Guest IPv4 gateway did not respond.'
printf 'Guest default gateway: responds\n'
for endpoint in 'https://1.1.1.1/cdn-cgi/trace' 'https://www.apple.com/library/test/success.html'; do
    /usr/bin/curl -q -4 -fsS --noproxy '*' --max-time 12 -o /dev/null "$endpoint" ||
        fail "Guest HTTPS failed for $endpoint"
    printf 'Guest HTTPS %s: works\n' "$endpoint"
done

[[ -f "$archive" ]] || fail 'The qualification ZIP is missing in the guest.'
actual_sha="$(/usr/bin/shasum -a 256 < "$archive")"
[[ "${actual_sha%% *}" == "$expected_sha" ]] || fail 'The qualification ZIP SHA-256 differs from the host.'
printf 'Qualification ZIP: SHA-256 verified\n'

work="$(/usr/bin/mktemp -d "${TMPDIR:-/tmp}/marsa-qualification.XXXXXXXX")"
trap '/bin/rm -rf -- "$work"' EXIT
/usr/bin/ditto -x -k "$archive" "$work"
app="$work/Marsa VM Qualification.app"
/usr/bin/codesign --verify --deep --strict "$app"
[[ "$(/usr/sbin/spctl --status 2>&1)" == 'assessments enabled' ]] ||
    fail 'Gatekeeper assessments must be enabled.'
/usr/bin/xattr -w com.apple.quarantine "0081;$(printf '%x' "$(/bin/date +%s)");MarsaQualification;" "$app"
/usr/sbin/spctl --assess --type execute "$app" || fail 'Marsa Gatekeeper assessment: rejected'
printf 'Marsa Gatekeeper assessment: accepted with a test quarantine marker\n'

/usr/bin/open "$app"
for ((attempt = 0; attempt < 60; attempt++)); do
    if [[ -S "$HOME/.marsa/run/marsad.sock" ]]; then
        printf 'Marsa first launch: daemon socket ready\n'
        exit 0
    fi
    /bin/sleep 1
done
fail 'Marsa first launch: daemon socket did not become ready.'
