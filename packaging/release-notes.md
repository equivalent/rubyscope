Rubyscope {{VERSION}}. Alpha software: expect rough edges.

## Install

**Linux** (x86_64, built on Arch Linux):

```
tar xzf rubyscope-{{VERSION}}-linux-x86_64.tar.gz
rubyscope-{{VERSION}}-linux-x86_64/install.sh
```

It installs `~/.local/bin/rubyscope`, `~/.local/bin/rubyscope-cli`, the launcher entry and the icons. Make sure `~/.local/bin` is on your `PATH`.

**macOS** (Apple silicon, macOS 11 or newer): open `Rubyscope-{{VERSION}}-macos-arm64.dmg` and drag Rubyscope to Applications. The app is not signed, so run this once before the first start:

```
xattr -dr com.apple.quarantine /Applications/Rubyscope.app
```

For the terminal command:

```
ln -sf /Applications/Rubyscope.app/Contents/MacOS/rubyscope ~/.local/bin/rubyscope
```

## Use

```
rubyscope ~/code/my-rails-app
```

The project needs its gems installed (`bundle install`). See the README for keys and settings.
