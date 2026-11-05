#!/usr/bin/env bash
# Wrap a release binary in a minimal macOS .app bundle (retina-capable, dark-mode aware), written
# next to the binary as Terminal.app: target/release/terminal unless another path is given.
# The executable inside is named Terminal too, since the app menu's About, Hide and Quit items
# take the process name. Independent builds do NOT install Sparkle by default:
# without our own update-signing key, they must never trust the original project's updates.
# Supply TR_SPARKLE_PUBLIC_KEY (our own Ed25519 public key) to opt into an update-enabled bundle.
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
bin=${1:-target/release/terminal}
app=$(dirname "$bin")/Terminal.app
# The bundle ID keeps the project's old name. Released copies have it, and macOS ties the app's
# Keychain access, privacy permissions and preferences (Sparkle's among them) to it, so changing
# it would make the next update a different app as far as macOS is concerned. Nothing shows it.
bundle_id=dev.terminal-rs.app
# Never reuse the upstream project's Sparkle signing key. The custom public key
# is deliberately unset by default until the independent repo has its own keypair.
feed=https://github.com/cantusprote/terminal-rs/releases/latest/download/appcast.xml
sparkle_key=${TR_SPARKLE_PUBLIC_KEY:-}
if [[ -n "$sparkle_key" && ! "$sparkle_key" =~ ^[A-Za-z0-9+/]{43}=$ ]]; then
  echo "TR_SPARKLE_PUBLIC_KEY must be a base64-encoded Ed25519 public key" >&2
  exit 1
fi
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/Terminal"
cp app/assets/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
sparkle_plist=""
if [[ -n "$sparkle_key" ]]; then
  sparkle=$(scripts/fetch-sparkle.sh)
  mkdir -p "$app/Contents/Frameworks"
  # The app isn't sandboxed, so Sparkle's XPC services aren't needed.
  framework=$app/Contents/Frameworks/Sparkle.framework
  ditto "$sparkle/Sparkle.framework" "$framework"
  for part in XPCServices Headers PrivateHeaders Modules; do
    rm -rf "${framework:?}/$part" "${framework:?}/Versions/B/$part"
  done
  sparkle_plist=$(cat <<KEYS
  <key>SUFeedURL</key><string>${feed}</string>
  <key>SUPublicEDKey</key><string>${sparkle_key}</string>
  <key>SURequireSignedFeed</key><true/>
  <key>SUVerifyUpdateBeforeExtraction</key><true/>
  <key>SUEnableAutomaticChecks</key><true/>
KEYS
)
fi
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Terminal</string>
  <key>CFBundleDisplayName</key><string>Terminal</string>
  <key>CFBundleIdentifier</key><string>${bundle_id}</string>
  <key>CFBundleExecutable</key><string>Terminal</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.finance</string>
${sparkle_plist}
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null
echo "bundled $app"
