---
name: release
description: Use when the user asks for a release of Rubyscope ("do a release", "make a release", "release 0.0.2", "/release"). Audits the repo and its history for private names and secrets, builds the Linux tar.gz and the macOS DMG, tags the version and makes a GitHub pre-release. The request is the permission to commit the version change, push main, push the tag and publish the release.
---

# release — publish a Rubyscope release

`bin/release <version>` does the work. This skill adds what a script cannot
do: pick the version, look at pictures, and read the result.

## Before

1. **Version.** Use the version the user gives. If none, take the last tag
   (`git tag -l 'v*' --sort=-v:refname | head -1`) and add 1 to the last
   number (0.0.1 gives 0.0.2). Stay on 0.0.x until the user asks for more.
2. **Mac host.** `RUBYSCOPE_MAC_HOST` must be the ssh host of a Mac with
   Rust. Take it from the user's own instructions (their list of machines).
   Never write the host name into a repo file.
3. **Private words.** `bin/release` reads `~/.config/rubyscope/private-words`
   (outside the repo). If it is missing, stop and ask the user for the
   names of their private projects, people and machines. Never commit it.
4. **Main is ready.** Clean tree on `main`, same as `origin/main`. If work
   is not committed, stop and ask.
5. **Pictures.** The text scan cannot read images. List the images that
   changed since the last tag
   (`git diff --name-only <last tag> HEAD -- '*.png' '*.webp' '*.jpg'`), and
   look at each one. Only made-up example code may be in them. No real
   project, path, person or data.
6. **New docs and PR text.** Read the diff of `*.md` and `webpage/` since the
   last tag, and the PRs merged since then. Check the rules in AGENTS.md:
   no file count, gem count or memory use of a real app, no machine names.

If any check finds something, fix it in a commit first. Changes to old
history (a rewrite and force push) are only for the user to decide.

## Run

```
RUBYSCOPE_MAC_HOST=<user@mac> bin/release <version>
```

Run it in the background with a long timeout; a clean build takes about
10 minutes. It stops at the first failed step and says which.

If it stops after the "Release <version>" commit was pushed, fix the cause
and run it again with the same version. The commit step is skipped when the
version is already set. If the tag exists but the GitHub release does not,
run only the `gh release create` line at the end of the script by hand.

## After

1. `gh release download v<version> -D <empty temp dir>` and compare the
   sha256 of both files with the ones in `dist/`.
2. Report the release URL, the two file names, and say that the scan and
   `bin/ci` passed. Say that the app windows were not opened, unless they were.
