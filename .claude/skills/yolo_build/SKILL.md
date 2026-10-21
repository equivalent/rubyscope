---
name: yolo_build
description: Use when the user says "yolo rebuild", "yellow rebuild", "yolo build" or runs /yolo_build. Pulls the latest main of this repo, rebuilds the app for this computer (Linux on Linux, macOS on macOS) and installs it as the current executable. The phrase is the permission — do not ask first.
---

# yolo_build — pull main, rebuild rubyscope, install it here

Saying "yolo rebuild" (or "yellow rebuild", the same thing) is Tomas's permission to do all of
these steps. Do not ask first. This changes only the local install. It never pushes or commits.

## Steps

1. **Go to the repo root.** `cd "$(git rev-parse --show-toplevel)"`. This skill is part of the
   repo, so the repo is the one you are in.
2. **Check the branch and changes.** Run `git status --short --branch`.
   - Not on `main`, or uncommitted changes: do not switch branch or discard anything. Stop and
     tell Tomas what is there.
3. **Pull.** `git pull --ff-only`. If it fails, stop and show the error. Do not merge or reset.
4. **Pick the installer from the OS.** Run `uname -s`.
   - `Linux`: `bin/install`. It installs `~/.local/bin/rubyscope`, `~/.local/bin/rubyscope-cli`,
     the `.desktop` menu entry and the icons.
   - `Darwin`: `bin/install-macos`. It installs `~/Applications/rubyscope.app` and links
     `~/.local/bin/rubyscope` to it.
   - Run it with a long timeout (up to 10 minutes). A first build is slow.
5. **Check the result.** `ls -l` the installed files. On Linux, also check that
   `~/.local/share/applications/rubyscope.desktop` exists. If the build fails, show the error
   lines and stop.
6. **Report.** Say the OS, the commit now at `HEAD` (`git log --oneline -1`), the commits that
   came in (`git log --oneline <old>..HEAD`, short), and that an open window still runs the old
   build until it is opened again.

## Notes

- The tests do not run here. Use `bin/ci` for that.
- Keep the report short and plain.
