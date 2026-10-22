# Security policy

Repeat That keeps captured audio in memory only, and has no account and no telemetry. Its one network feature is an
update check that stays off until the user allows it, and that installs a new version only when it is signed with the
project's update key. See [PRIVACY.md](PRIVACY.md#update-checks). The realistic security bugs are therefore in the app
itself: another program being able to read the audio buffer, captured audio ending up on disk, another app's volume
being left lowered, or the updater installing a file it shouldn't. The one thing capture does write to disk is the duck
journal, which holds app volumes only, so that file is expected.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting: [report a vulnerability](https://github.com/Roboroads/repeat-that/security/advisories/new).
The report is visible to the maintainer and to anyone the maintainer adds to the advisory. Please don't open a public
issue or pull request for a vulnerability, and keep the details private until a fix is released.

Say which version and operating system you tested, how to reproduce the problem, and what an attacker would gain. A
proof of concept helps but isn't required.

## Scope

In scope:

- the app
- the capture backends (WASAPI on Windows, PipeWire on Linux and Core Audio on macOS)
- the duck guard process and the duck journal
- the update check and installer, and the way the update signing key is kept and used
- release artifacts and their checksums (from v0.1)

Bugs in a third-party dependency belong upstream unless Repeat That uses it in a way that makes it exploitable.
Dependabot alerts are on for known advisories in our dependencies. Recording laws are not a security matter, see
[PRIVACY.md](PRIVACY.md).

## If the update signing key leaks

Releases carry files signed with an update signing key, and the app installs an update only if it is signed with that
key. Its private half is a secret that only the release workflow can use, plus a copy the maintainer keeps. If it
leaks, a thief still needs a way to hand a file to the app, which reads its updates from this repository's releases
over HTTPS. But the old key can't be taken out of apps that already have it. The fix is a new key and a release with
the new public key in it. People on the old key then get a "Couldn't install" message with a link to the release page,
and have to install that release by hand. The maintainer would say so in a security advisory. How the key is kept and
replaced is in [MAINTAINING.md](docs/MAINTAINING.md#rotating-or-losing-the-key).

## Supported versions

Repeat That is pre-1.0, and the first release is v0.1.0. Only the latest release gets security fixes. If a problem
affects an older release, the answer is to update.

## What to expect

This is a one-person project. A report gets an acknowledgement within a week. After that the maintainer will say
whether it is accepted and when a fix is likely. Serious problems are fixed first, but there is no guaranteed
turnaround. Once a fix is out it is published as a GitHub security advisory, and you are credited there unless you ask
not to be.

There is no bug bounty.
