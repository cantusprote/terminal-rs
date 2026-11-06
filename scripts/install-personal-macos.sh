#!/usr/bin/env bash
# Install private Terminal-rs GUI + standalone CLI on this Mac.
# No public release, external downloads, notarization or privileged commands.
set -euo pipefail
cd "$(dirname "$0")/.."

# Personal apps live in the user's Applications folder. Override explicitly
# with TR_INSTALL_DIR if another destination is required.
install_dir="${TR_INSTALL_DIR:-$HOME/Applications}"
destination="$install_dir/Terminal-rs.app"
executable="$destination/Contents/MacOS/Terminal"
if pgrep -f "^$executable$" >/dev/null 2>&1; then
  echo "Terminal-rs is running. Quit the app before replacing its bundle." >&2
  exit 1
fi

cargo build --release -p terminal -p terminal-cli
scripts/bundle-macos.sh target/release/terminal
mkdir -p "$install_dir"
stage=$(mktemp -d "$install_dir/.terminal-rs-stage.XXXXXX")
cleanup() { rm -rf "$stage"; }
trap cleanup EXIT
bundle="$stage/Terminal-rs.app"
ditto target/release/Terminal.app "$bundle"
plist="$bundle/Contents/Info.plist"
/usr/libexec/PlistBuddy -c 'Set :CFBundleName Terminal-rs' "$plist"
/usr/libexec/PlistBuddy -c 'Set :CFBundleDisplayName Terminal-rs' "$plist"
/usr/libexec/PlistBuddy -c 'Add :LSEnvironment dict' "$plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c 'Add :LSEnvironment:TR_APPLE_STOCKS_CACHE string 1' "$plist" 2>/dev/null \
  || /usr/libexec/PlistBuddy -c 'Set :LSEnvironment:TR_APPLE_STOCKS_CACHE 1' "$plist"
# Reading the local cache is useful, but launching Stocks to fill missing
# symbols must not override the user's Cmd+Q dismissal of Stocks.
/usr/libexec/PlistBuddy -c 'Add :LSEnvironment:TR_APPLE_STOCKS_LAUNCH string 0' "$plist" 2>/dev/null \
  || /usr/libexec/PlistBuddy -c 'Set :LSEnvironment:TR_APPLE_STOCKS_LAUNCH 0' "$plist"
plutil -lint "$plist"
# Ad-hoc signing is sufficient for this personal local installation.
codesign --force --deep --sign - "$bundle"
codesign --verify --deep --strict "$bundle"

previous="$stage/previous.app"
if [[ -e "$destination" ]]; then mv "$destination" "$previous"; fi
if ! mv "$bundle" "$destination" || ! codesign --verify --deep --strict "$destination"; then
  rm -rf "$destination"
  if [[ -e "$previous" ]]; then mv "$previous" "$destination"; fi
  echo "Installation failed; original app restored." >&2
  exit 1
fi
# Keep an equally configured build artifact as a convenient copy for this Mac.
project_app="$(pwd)/target/release/Terminal-rs.app"
if pgrep -f "^$project_app/Contents/MacOS/Terminal$" >/dev/null 2>&1; then
  echo "Preserving currently running $project_app; use $destination after quitting."
else
  rm -rf target/release/Terminal-rs.app
  ditto "$destination" target/release/Terminal-rs.app
fi
echo "Installed: $destination"
echo "CLI: $(pwd)/target/release/terminal-cli"
echo "Apple Stocks cache read-only; automatic launch disabled. No public updates or trade execution."
