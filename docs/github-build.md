# Building on GitHub

The app can be built by GitHub Actions in your own fork of this
repository, with your own `game.wasm`, without installing anything. Nothing
built from the game is published: the game's code goes into a **draft
release** of your fork, and the apps come back into the same draft. A
draft is seen only by you (and by anyone you give write access to the
fork); to everyone else, logged in or not, it does not exist.

## Once

1. **Fork** this repository (forks of a public repository are public:
   that is what makes GitHub's macOS and Linux machines free). In the
   fork's **Actions** tab, enable the workflows.
2. In the fork, **Releases › Draft a new release**. Attach `game.wasm`
   (the supported build, see the [README](../README.md)), give it any
   title and press **Save draft**. Never **Publish release**.

## Each build

**Actions › Build › Run workflow**, choose the platform, **Run
workflow**. It checks `game.wasm` first and stops within a minute if the
draft or the file is missing or wrong, saying what to do. When it ends,
the app is in the draft, next to `game.wasm`:

| Platform | File | Machine |
|---|---|---|
| iOS | `Glue5-ios.ipa`, unsigned | macOS |
| Android | `Glue5-android.apk`; `Glue5-android.keystore` is its signing key | Linux |
| macOS | `Glue5-macos.dmg`, unsigned | macOS |
| Linux | `Glue5-linux-x86_64.AppImage`, `Glue5-linux-aarch64.AppImage` | Linux |

The workflow runs the same tasks as a build on a computer (`mise run
ios:ipa`, `android:build`, `macos:package`, `linux:container`), with the
toolchain in `.mise/config.toml`. A job may last up to six hours.

To build a newer version: **Sync fork** on the fork's page, then run the
workflow again. `game.wasm` stays in the draft.

## Installing

- **iOS:** sign and install the IPA with a sideloading tool (Sideloadly,
  AltStore, SideStore) and your Apple ID. With a free Apple ID the app
  runs for 7 days, then is installed again. The IPA asks for the
  increased memory limit (`platform/ios/Glue5.entitlements`), which free
  accounts may have.
- **Android:** install the APK. The first build makes a signing key and
  keeps it in the draft (`Glue5-android.keystore`); later builds sign
  with it, so they install over the app. Do not delete it: an APK signed
  with another key has to replace the app, and that deletes its game
  folder.
- **macOS:** the app is not signed for distribution: Gatekeeper blocks it
  until you allow it (System Settings › Privacy & Security, or
  `xattr -dr com.apple.quarantine "Glue 5.app"`).
- **Linux:** make the AppImage executable and run it.

The game's files then go on the device as described in the
[README](../README.md#platforms), or stream from a computer.

## If a release is published

Published, a release's files can be downloaded by anyone. The **Release
guard** workflow puts a release with a `.wasm` or a `Glue5-*` file back to
draft as soon as it is published or edited (files added included), and
fails, so that GitHub notifies you. That takes a few seconds (about 8 in
our tests), during which the files are public. The Build workflow uploads
only into a draft and refuses to start while a published release has
them.

## Caches

The workflows keep no cache of the build: GitHub lets pull requests from
other people's forks read the caches of a repository's default branch,
and the build's objects are made from the game's code. Only the toolchain
(mise's tools) is cached.
