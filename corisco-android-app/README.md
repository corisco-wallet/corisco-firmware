# Corisco mobile app

React Native (Expo SDK 57) wallet app that pairs with the firmware
hardware signer over BLE and never holds private key material itself (see
`App.tsx`'s top doc comment). This README covers building and running the
Android app locally; it assumes a real ESP32 signer for anything past the
pairing screen, but the app builds and launches fine without one.

> Expo versions change fast. If something here doesn't match what you see,
> check the exact-versioned docs at https://docs.expo.dev/versions/v57.0.0/
> before assuming this file is wrong (see `AGENTS.md`).

## Prerequisites

- **Node.js** (any recent LTS) and npm.
- **JDK 17** -- Android Gradle builds require it specifically (not 21, not 11).
- **Android SDK** -- cmdline-tools, `platform-tools`, `platforms;android-36`,
  `build-tools;36.0.0` (the versions matching Expo SDK 57 / this project's
  `compileSdkVersion`).
- A **physical Android device** with USB debugging enabled, or an emulator.
  BLE doesn't work in the emulator, so pairing/signing needs a real phone.

If you don't already have a JDK 17 + Android SDK set up (no Android Studio
required -- the cmdline-tools alone are enough), the short version:

```bash
# JDK 17 (adjust the download for your platform/arch)
mkdir -p ~/.local/opt && cd ~/.local/opt
curl -L -o jdk17.tar.gz "<a Temurin JDK 17 tarball URL for your platform>"
tar xzf jdk17.tar.gz && mv jdk-17* jdk17

# Android SDK cmdline-tools
mkdir -p ~/Android/Sdk/cmdline-tools
cd ~/Android/Sdk/cmdline-tools
curl -L -o tools.zip "https://dl.google.com/android/repository/commandlinetools-linux-<version>_latest.zip"
unzip tools.zip && mv cmdline-tools latest

# Persist in ~/.bashrc (or your shell's rc file)
export JAVA_HOME="$HOME/.local/opt/jdk17"
export ANDROID_HOME="$HOME/Android/Sdk"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
export PATH="$JAVA_HOME/bin:$ANDROID_HOME/cmdline-tools/latest/bin:$ANDROID_HOME/platform-tools:$PATH"

# Accept licenses and install what's needed
yes | sdkmanager --licenses
sdkmanager "platform-tools" "platforms;android-36" "build-tools;36.0.0"
```

## Install dependencies

```bash
cd corisco-android-app
npm install
```

## Generate the native Android project

This app is managed-workflow Expo, so the `android/` folder is generated,
not hand-edited. Run this once, and again any time a native dependency is
added or `app.json`'s native config changes:

```bash
npx expo prebuild --platform android
```

(Adding a plain npm dependency with no native code -- most JS-only changes
-- does *not* need this; Gradle's autolinking picks up new native modules
from `node_modules` on its own at build time.)

## Running in debug mode

Debug builds load their JS bundle from a running Metro server rather than
bundling it in, so you get fast refresh instead of a full rebuild per
change.

1. Build and install the debug client onto a connected device:

   ```bash
   npx expo run:android
   ```

   This builds `android/app/build/outputs/apk/debug/app-debug.apk` and
   installs it via `adb` in one step. If `adb` can't see your device (a
   known flaky spot under WSL2 -- see Troubleshooting), build the APK
   separately and sideload it instead (see below), then just start Metro:

   ```bash
   npx expo start --dev-client
   ```

2. Open the installed app on the device. On first launch it'll ask for the
   Metro server address (auto-detected on the same network, or over USB
   via `adb reverse tcp:8081 tcp:8081`).

3. Edit code, save -- the app hot-reloads. Shake the device (or run
   `adb shell input keyevent 82`) to open the dev menu (reload, inspector,
   etc).

## Building a debug APK (no install)

```bash
cd android
./gradlew assembleDebug
```

Output: `android/app/build/outputs/apk/debug/app-debug.apk`. This build
still needs Metro running (`npx expo start`) to load its JS -- it is not
standalone.

## Building a release APK (standalone)

```bash
cd android
./gradlew assembleRelease
```

Output: `android/app/build/outputs/apk/release/app-release.apk`. This
build embeds the JS bundle and is minified -- no Metro server needed, just
install and run.

> This project's `android/app/build.gradle` currently reuses the **debug
> keystore** for release signing too. That's fine for installing on your
> own device, but it is explicitly **not** Play-Store-ready -- production
> distribution needs a dedicated release keystore
> (`android/app/build.gradle`'s own comment flags this).

A first build of either variant takes several minutes (Gradle + native
compilation, cold cache); subsequent builds are much faster.

## Installing an APK on a device

```bash
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
# or the release path, respectively
```

If `adb` can't reach the device (seen under WSL2 -- USB passthrough to a
Linux VM can be unreliable), copy the APK file to the phone by another
means (USB file transfer / drag-and-drop in a file manager works well; avoid
large-file transfer services that can silently truncate the file -- verify
with `sha256sum` on both ends if a "problem parsing the package" error
shows up on install) and install it from the phone's file manager instead.

## Troubleshooting

- **"bleerror: device is not authorized to use BluetoothLE"** -- Android
  12+ requires runtime Bluetooth permissions; the app requests these on
  connect, but if you denied them once, grant `Nearby devices` manually in
  the app's system settings page.
- **"No device named 'SparkHW' found"** -- make sure the signer is powered
  on and advertising (not already connected to another phone/app). If it
  was working before and suddenly isn't, try toggling the phone's
  Bluetooth off/on -- Android throttles repeated BLE scans, which produces
  this exact symptom with no other error.
- **`adb` hangs or can't see the device (WSL2)** -- check
  `/dev/bus/usb/*` permissions (`sudo chmod 666 /dev/bus/usb/<bus>/<dev>`),
  and if the device drops out mid-session, reattach it via `usbipd` on the
  Windows host rather than just unplugging/replugging.
