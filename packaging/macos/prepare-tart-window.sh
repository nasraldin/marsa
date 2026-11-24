#!/bin/bash
# Give the qualification window a distinct identity from other Tart instances.
set -euo pipefail

if [[ $# != 2 || "$1" != /*.app || "$2" != /*.app ]]; then
    printf 'Usage: %s /absolute/path/to/Tart.app /absolute/path/to/Qualification.app\n' "$0" >&2
    exit 2
fi
source_app="$1"
destination_app="$2"
bundle_id=io.github.nasraldin.marsa.tart-qualification-viewer
if [[ -e "$destination_app" || -L "$destination_app" ]]; then
    printf 'The qualification app destination must be unused.\n' >&2
    exit 1
fi
/usr/bin/codesign --verify --deep --strict \
    --test-requirement '=anchor apple generic and identifier "com.github.cirruslabs.tart"' "$source_app"

parent="$(/usr/bin/dirname -- "$destination_app")"
work="$(/usr/bin/mktemp -d "$parent/.tart-qualification.XXXXXXXX")"
trap '/bin/rm -rf -- "$work"' EXIT
app="$work/Qualification.app"
entitlements="$work/entitlements.plist"
/usr/bin/ditto "$source_app" "$app"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $bundle_id" "$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c 'Set :CFBundleName Marsa Tart Qualification' "$app/Contents/Info.plist"

# Only VM control needs an entitlement. Test networking uses --net-fd.
cat > "$entitlements" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>com.apple.security.virtualization</key><true/></dict></plist>
PLIST
/usr/bin/codesign --force --sign - --entitlements "$entitlements" "$app"
/usr/bin/codesign --verify --deep --strict --test-requirement "=identifier \"$bundle_id\"" "$app"
/bin/mv -h -n "$app" "$destination_app"
[[ ! -d "$app" ]] || { printf 'The destination was created concurrently.\n' >&2; exit 1; }
printf '%s\n' "$destination_app/Contents/MacOS/tart"
