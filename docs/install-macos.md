# Installing on macOS

Repeat That is a free, volunteer-run project, and we don't pay Apple's $99/year for a Developer ID. The macOS builds are
therefore **ad-hoc signed but not notarized**. They work fine, but the first time you open the app macOS will say it
"cannot verify" the app or that it "is damaged". That message comes from Gatekeeper, not from a problem with the app.

If you'd rather not trust a pre-built binary, [build it yourself](#build-from-source). Apps you build locally are not
quarantined, so Gatekeeper never complains.

## Option 1 — "Open Anyway" (no Terminal)

1. Download the `.dmg` from the [Releases page](https://github.com/Roboroads/repeat-that/releases) and drag
   **Repeat That** into **Applications**.
2. Open the app. macOS blocks it. Click **Done** (or **OK**), not "Move to Trash".
3. Open **System Settings → Privacy & Security** and scroll down to **Security**. You'll see *"Repeat That was
   blocked to protect your Mac"*. Click **Open Anyway** and confirm with your password or Touch ID.
4. Open the app again and click **Open** in the dialog that follows.

You only need to do this once per version.

> On macOS 14 and earlier you can also right-click (Control-click) the app in Finder → **Open** → **Open**. macOS 15
> removed this shortcut, so use the steps above there.

## Option 2 — Remove the quarantine flag (Terminal)

macOS marks downloaded files with a "quarantine" attribute. Removing it from the installed app skips the Gatekeeper
check:

```sh
xattr -dr com.apple.quarantine "/Applications/Repeat That.app"
```

Use this if you get *"is damaged and can't be opened"*, which Option 1 doesn't always fix.

## Verify your download

Each release publishes a `SHA256SUMS` file. Check that your download matches before you bypass Gatekeeper:

```sh
shasum -a 256 ~/Downloads/Repeat-That_*_universal.dmg
```

Compare the output with the matching line in `SHA256SUMS` on the release page.

## Permissions

Repeat That needs permission to capture other apps' audio. The first time you start capturing, macOS asks for it.
If you clicked "Don't Allow", turn it back on in **System Settings → Privacy & Security** under audio capture
(**Screen & System Audio Recording** on recent versions).

Ad-hoc signed apps get a new signature with every release, so **macOS may ask for this permission again after an
update**. That's expected. If capture silently stops working after an update, remove the old entry from the list and
start capturing again.

## Build from source

```sh
xcode-select --install          # Apple's command-line tools
curl https://sh.rustup.rs -sSf | sh
git clone https://github.com/Roboroads/repeat-that && cd repeat-that
# once the desktop app lands (milestone 4):
cd app && npm install && npm run tauri build
```

The built app ends up in `target/release/bundle/macos/`.
