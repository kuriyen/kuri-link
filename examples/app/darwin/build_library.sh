#!/bin/sh
# The app's one native library, the Kuri Link bindings and the echo back end,
# built by Cargo from examples/echo and put in the app bundle, as
# linux/CMakeLists.txt and android/app/build.gradle.kts do for their platforms.
# A build phase of the Runner target runs it for macOS and for iOS, so it takes
# what to build from Xcode's environment: the platform, the architectures, the
# configuration, the deployment target and the identity to sign with.
#
# macOS loads a plain library from Contents/Frameworks. iOS loads only
# frameworks, so there the library is the binary of kuri_link_echo.framework.
set -eu

# Xcode's environment, when Xcode itself was started from the Dock, does not
# have rustup's directory on its PATH.
PATH="$PATH:$HOME/.cargo/bin"

root="$SRCROOT/../../.."
case "$CONFIGURATION" in
  Debug) profile=dev out=debug ;;
  *) profile=release out=release ;;
esac

targets=""
libraries=""
for arch in $ARCHS; do
  case "$PLATFORM_NAME/$arch" in
    macosx/arm64) target=aarch64-apple-darwin ;;
    macosx/x86_64) target=x86_64-apple-darwin ;;
    iphoneos/arm64) target=aarch64-apple-ios ;;
    iphonesimulator/arm64) target=aarch64-apple-ios-sim ;;
    iphonesimulator/x86_64) target=x86_64-apple-ios ;;
    *) echo "error: no Rust target for $arch on $PLATFORM_NAME" >&2; exit 1 ;;
  esac
  targets="$targets --target $target"
  libraries="$libraries $root/target/$target/$out/libkuri_link_echo.dylib"
done

# Build as from a terminal. Xcode puts its own clang first on the PATH, which
# finds an SDK only through SDKROOT, and SDKROOT is the app's platform: wrong
# for the build scripts Cargo compiles to run on this Mac. /usr/bin/cc asks
# xcrun for the SDK instead, and rustc and cc name each target's SDK
# themselves. The deployment targets stay, since rustc and cc must agree on
# them or the link fails; but an iOS build drops the macOS one, which Xcode
# sets to its SDK's version, newer than the Mac the build scripts run on.
PATH="/usr/bin:$PATH"
unset SDKROOT
if [ "$PLATFORM_NAME" != macosx ]; then
  unset MACOSX_DEPLOYMENT_TARGET
fi
# shellcheck disable=SC2086 # $targets is a list of arguments.
(cd "$root" && cargo build --locked -p kuri-link-echo --lib --profile "$profile" $targets)

frameworks="$TARGET_BUILD_DIR/$FRAMEWORKS_FOLDER_PATH"
mkdir -p "$frameworks"
if [ "$PLATFORM_NAME" = macosx ]; then
  bundle="$frameworks/libkuri_link_echo.dylib"
  binary="$bundle"
  name="@rpath/libkuri_link_echo.dylib"
else
  bundle="$frameworks/kuri_link_echo.framework"
  binary="$bundle/kuri_link_echo"
  name="@rpath/kuri_link_echo.framework/kuri_link_echo"
  rm -rf "$bundle" && mkdir -p "$bundle"
  cat > "$bundle/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleExecutable</key>
	<string>kuri_link_echo</string>
	<key>CFBundleIdentifier</key>
	<string>io.github.kuriyen.kuri-link-echo</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>kuri_link_echo</string>
	<key>CFBundlePackageType</key>
	<string>FMWK</string>
	<key>CFBundleShortVersionString</key>
	<string>1.0</string>
	<key>CFBundleVersion</key>
	<string>1</string>
	<key>MinimumOSVersion</key>
	<string>$IPHONEOS_DEPLOYMENT_TARGET</string>
</dict>
</plist>
PLIST
fi
# shellcheck disable=SC2086 # $libraries is a list of files.
lipo -create $libraries -output "$binary"
# Cargo names the library by the path it built it at; the app finds it by rpath.
install_name_tool -id "$name" "$binary"

# Changing the name broke the linker's signature. Sign with the app's identity,
# or, when the app is not signed here, at least validly, since arm64 runs no
# unsigned code.
identity="-"
if [ "${CODE_SIGNING_ALLOWED:-NO}" = YES ] && [ -n "${EXPANDED_CODE_SIGN_IDENTITY:-}" ]; then
  identity="$EXPANDED_CODE_SIGN_IDENTITY"
fi
codesign --force --sign "$identity" "$bundle"
