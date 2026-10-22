# Verifying downloads

Starting with v0.1, each release on the [Releases page](https://github.com/Roboroads/repeat-that/releases) includes a
`SHA256SUMS` file with the SHA-256 hash of each installer. Compute the hash of the file you downloaded and compare it
with the line for the same file name in `SHA256SUMS`. The two must be identical. Letter case doesn't matter:
`Get-FileHash` prints uppercase hex and `SHA256SUMS` is lowercase.

The macOS `.dmg` (`Repeat-That_<version>_aarch64.dmg`) can arrive on the release some time after the other files,
because the macOS build is not allowed to hold up the release. When it does, `SHA256SUMS` is replaced by a copy with
one extra line for the `.dmg`. The existing lines never change, so a check you made earlier against the old
`SHA256SUMS` stays valid. If the `SHA256SUMS` you downloaded has no line for the `.dmg`, download it again.

Windows (PowerShell):

```powershell
Get-FileHash $HOME\Downloads\<file> -Algorithm SHA256
```

To let PowerShell do the comparison, paste the hash from `SHA256SUMS`. It prints `True` on a match, and `-eq` ignores
case:

```powershell
(Get-FileHash $HOME\Downloads\<file> -Algorithm SHA256).Hash -eq "<hash from SHA256SUMS>"
```

macOS:

```sh
shasum -a 256 ~/Downloads/Repeat-That_<version>_aarch64.dmg
```

Linux:

```sh
sha256sum ~/Downloads/<file>
```

On macOS and Linux the tool can also do the comparison. Save `SHA256SUMS` in the same folder as the download, then run
one of these there:

```sh
sha256sum -c SHA256SUMS --ignore-missing       # Linux
shasum -a 256 -c SHA256SUMS --ignore-missing   # macOS
```

Your file must show `OK`. Listed files you didn't download are skipped. If your `shasum` doesn't know
`--ignore-missing`, check just the line of the `.dmg` instead:

```sh
grep -F Repeat-That_<version>_aarch64.dmg SHA256SUMS | shasum -a 256 -c
```

`SHA256SUMS` sits on the same release page as the installer, so this catches a corrupted or incomplete download. It
doesn't prove who built the file.

## macOS: check the signature (optional)

The `.dmg` is signed with a Developer ID certificate and notarized by Apple, so macOS can tell you who signed it and
that Apple's notary service accepted it. Unlike the checksum, that doesn't depend on the release page. Run this on the
download, and then on the app after you have copied it to Applications:

```sh
spctl -a -t open --context context:primary-signature -v ~/Downloads/Repeat-That_<version>_aarch64.dmg
spctl -a -t exec -vv "/Applications/Repeat That.app"
```

Each command must say `accepted` and `source=Notarized Developer ID`. The second one also prints an `origin=` line with
the name and team of the certificate that signed the app, which should be the maintainer's. Opening the app from
Applications does the same check, so this is only for people who want to see it.
