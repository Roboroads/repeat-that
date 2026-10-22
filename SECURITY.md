# Security policy

Repeat That keeps captured audio in memory only, has no account and no telemetry, and has no network features yet (a
Tauri auto-updater that downloads and installs new builds is planned, see the [roadmap](ROADMAP.md)). See
[PRIVACY.md](PRIVACY.md). That means the realistic security bugs are in the app itself: another program being able to
read the audio buffer, captured audio ending up on disk, or another app's volume being left lowered. The one thing
capture does write to disk is the duck journal, which holds app volumes only, so that file is expected.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting: [report a vulnerability](https://github.com/Roboroads/repeat-that/security/advisories/new).
The report is visible to the maintainer and to anyone the maintainer adds to the advisory. Please don't open a public
issue or pull request for a vulnerability, and keep the details private until a fix is released.

Say which version and operating system you tested, how to reproduce the problem, and what an attacker would gain. A
proof of concept helps but isn't required.

## Scope

In scope:

- the app
- the capture backends (WASAPI on Windows today, PipeWire and Core Audio once they exist)
- the duck guard process and the duck journal
- the auto-updater, once it exists
- release artifacts and their checksums (from v0.1)

Bugs in a third-party dependency belong upstream unless Repeat That uses it in a way that makes it exploitable.
Dependabot alerts are on for known advisories in our dependencies. Recording laws are not a security matter, see
[PRIVACY.md](PRIVACY.md).

## Supported versions

Repeat That is pre-1.0, and the first release is v0.1.0. Only the latest release gets security fixes. If a problem
affects an older release, the answer is to update.

## What to expect

This is a one-person project. A report gets an acknowledgement within a week. After that the maintainer will say
whether it is accepted and when a fix is likely. Serious problems are fixed first, but there is no guaranteed
turnaround. Once a fix is out it is published as a GitHub security advisory, and you are credited there unless you ask
not to be.

There is no bug bounty.
