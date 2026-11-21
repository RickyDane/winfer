
---
### Install

**macOS** — open the `.dmg` for your Mac (`aarch64` = Apple silicon, `x64` = Intel) and drag winfer to Applications. Signed with a Developer ID and notarized by Apple.

**Windows** — run the `-setup.exe` (or the `.msi`); SmartScreen warns because the installer isn't signed: More info → Run anyway.

**Linux** — the `.AppImage` runs anywhere (`chmod +x`, then start it) and updates itself; or install the `.deb` (Debian/Ubuntu) or `.rpm` (Fedora/openSUSE). Needs WebKitGTK 4.1; `notify-send` (libnotify) for actionable notifications.

**winfer native** (the GPUI build, `winfer-native-*`; preview) — macOS: the `.dmg` for your Mac; Windows: the portable `.exe`; Linux: the `.tar.gz` (unpack, run `./winfer`; needs WebKitGTK 4.1). It updates itself on the channel picked in its settings.
