# SnapClip

A lightweight screenshot tool and clipboard manager for Windows (Linux supported, Windows first).
It is written in Rust with egui. It runs in the system tray, starts with Windows, and opens windows only when you need them.

## Features

**Screenshots** (default shortcut `Alt+Shift+S`, or left-click the tray icon)
- Select a rectangle, ellipse or freehand area, or click once for the full screen.
- Resize and move the selection with handles or the arrow keys. A magnifier shows pixel coordinates and color.
- Annotate with pen, highlighter, line, arrow, rectangle, ellipse (outline or filled), text, numbered steps and pixelate (to hide private info).
- Pick a color (10 presets or any custom color) and a thickness or font size. Undo/redo with `Ctrl+Z` / `Ctrl+Y`.
- Copy (`Enter` / `Ctrl+C`), save to a folder (`Ctrl+S`), save as (`Ctrl+Shift+S`), or pin on screen (`Ctrl+P`).
- **Scrolling capture** (📜 button): captures a long page. On Windows the page scrolls automatically; you can also scroll by hand. Frames are stitched together, and sticky footers appear only once.
- After copying or saving, a notification with a preview appears. Click it to open the **editor**. In Settings you can switch this to open the editor right away, or do nothing.

**Clipboard history** (default shortcut `Alt+Shift+V`)
- Keeps text and images. Search, pin, edit, delete.
- `Enter` or a click pastes into the window you were using. `Ctrl+1…9` pastes the first nine items. `Shift+Del` deletes, `Ctrl+P` pins, `Ctrl+E` edits.

## Download

Get the latest Windows build from the [Releases page](https://github.com/muhammedaslan34/snapclip/releases).
Pick `SnapClip-<version>-windows-x64.exe`. It's a single file with nothing to install.
Put it in a permanent folder before the first run, because it registers itself to start with Windows from that location.
The file isn't code-signed yet, so SmartScreen will warn: click **More info → Run anyway**.

## Shortcuts

Every shortcut can be changed in **Settings → Keyboard shortcuts**. That includes the two global ones, the screenshot tools and actions, and the clipboard panel keys.
Type a combination (`Ctrl+Shift+S`, `F9`, `P`) or click **Record** and press it. Separate several shortcuts with commas, or leave a field empty to disable it.
Conflicts are flagged before you apply.

## Build

```sh
cargo run                 # debug build; the app stays in the tray
cargo build --release     # small optimized binary: target/release/snapclip(.exe)
cargo test
```

On Windows, install Rust from https://rustup.rs and run `cargo build --release`.

Cross-compiling the Windows `.exe` from Linux (needs clang/lld; downloads the MSVC CRT and Windows SDK libraries on first use):

```sh
cargo install --locked cargo-xwin
cargo xwin build --release --target x86_64-pc-windows-msvc
# → target/x86_64-pc-windows-msvc/release/snapclip.exe  (standalone, static C runtime)
```
The GitHub Actions workflow in `.github/workflows/build.yml` also builds `snapclip.exe` and uploads it as an artifact.

Startup flags: `--capture`, `--clipboard`, `--settings`. Each opens that window when the app starts.

## Where data lives

- Settings: `%APPDATA%\SnapClip\settings.json` (Linux: `~/.config/SnapClip/`)
- Clipboard history: `%LOCALAPPDATA%\SnapClip\` (Linux: `~/.local/share/SnapClip/`)
- Screenshots: `Pictures\SnapClip` by default

## Design notes

- Every window is created when it opens and destroyed when it closes, together with its OpenGL context. While idle, the app is only the tray icon, the hotkeys and a clipboard listener.
- The clipboard listener uses `AddClipboardFormatListener` on Windows, so it reacts to events instead of polling.
- Annotations are stored in screenshot pixel coordinates and rendered twice: live with egui, and into the final image with tiny-skia. Text uses the same font in both, so the saved file matches the screen.

## Linux limitations

- Global shortcuts work on X11 only. On Wayland, use the tray menu for now. Forwarding `--capture` to an already-running instance is not implemented yet.
- Auto-paste and auto-scroll are Windows-only. On Linux the item is copied and you paste it yourself.

## Releasing

1. Bump `version` in `Cargo.toml` (e.g. `0.2.0`) and commit.
2. Tag and push:
   ```sh
   git tag v0.2.0
   git push origin v0.2.0
   ```
3. `.github/workflows/release.yml` runs the tests, builds `snapclip.exe` on Windows, and publishes a GitHub Release with the `.exe`, a `.zip` and `SHA256SUMS.txt`. Release notes are generated from the commits.

The tag must match the `Cargo.toml` version, or the release job fails. A tag with a suffix, like `v0.2.0-beta.1`, is published as a pre-release.
