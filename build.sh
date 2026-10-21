#!/bin/sh
set -eu
cd "$(dirname "$0")"
app_name="homebrew-gui"

if [ "$(uname -s)" != Darwin ]; then
    printf '%s\n' 'build.sh requires macOS to create an .app bundle.' >&2
    exit 1
fi

if [ "$#" -gt 1 ]; then
    printf 'Usage: %s [aarch64-apple-darwin|x86_64-apple-darwin]\n' "$0" >&2
    exit 1
fi
target=${1:-$(rustup run stable rustc -vV | sed -n 's/^host: //p')}
case "$target" in
    aarch64-apple-darwin|x86_64-apple-darwin) ;;
    *) printf 'Unsupported macOS target: %s\n' "$target" >&2; exit 1 ;;
esac
version=$(sed -n 's/^version = "\([0-9.]*\)"$/\1/p' Cargo.toml)
[ -n "$version" ] || { printf '%s\n' 'Missing package version in Cargo.toml.' >&2; exit 1; }
rustup run stable cargo build --locked --release --target "$target" --target-dir target

bundle="target/release/$app_name.app"
if [ "$#" -eq 1 ]; then
    bundle="target/$target/release/$app_name.app"
fi
identifier="local.$(printf '%s' "$app_name" | tr '_' '-')"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp assets/app-icon.icns "$bundle/Contents/Resources/app-icon.icns"
cp "target/$target/release/$app_name" "$bundle/Contents/MacOS/$app_name.new"
mv "$bundle/Contents/MacOS/$app_name.new" "$bundle/Contents/MacOS/$app_name"
cat > "$bundle/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>$app_name</string>
<key>CFBundleDisplayName</key><string>$app_name</string>
<key>CFBundleIdentifier</key><string>$identifier</string>
<key>CFBundleExecutable</key><string>$app_name</string>
<key>CFBundleIconFile</key><string>app-icon.icns</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>$version</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSPrincipalClass</key><string>NSApplication</string>
</dict></plist>
PLIST
plutil -lint "$bundle/Contents/Info.plist"
codesign --force --sign - --timestamp=none "$bundle"
codesign --verify --deep --strict "$bundle"
printf 'Built %s\nOpen it with: open "%s"\n' "$bundle" "$bundle"
