# Package manager manifests

Staged manifests for getting Wireless Battery Tray into Windows package managers. Nothing in this
folder is used by the build; changes here don't trigger a release.

## winget

`winget/manifests/f/Faulik/WirelessBatteryTray/<version>/` mirrors the layout of
[microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs). The package installs the
release zip as a portable app, with `wireless-battery-tray` and `wbt` on the PATH.

First submission (once, by hand):

1. Fork `microsoft/winget-pkgs` to your account.
2. Check the manifest locally: `winget validate --manifest packaging\winget\manifests\f\Faulik\WirelessBatteryTray\0.1.6`
   and try it: `winget settings --enable LocalManifestFiles`, then
   `winget install --manifest packaging\winget\manifests\f\Faulik\WirelessBatteryTray\0.1.6`.
3. Copy the version folder to the same path in your fork and open a PR to `microsoft/winget-pkgs`
   (or run `wingetcreate submit <folder>`). Review takes a few days.

Later versions, automatically: create a classic personal access token with `public_repo` scope,
save it as the repository secret `WINGET_TOKEN`, and set the repository variable
`WINGET_AUTOSUBMIT` to `true`. The `winget` job in `release.yml` then opens the winget-pkgs PR for
every release.

## Scoop

`scoop/wireless-battery-tray.json` is ready for the
[Extras bucket](https://github.com/ScoopInstaller/Extras) (it auto-updates there from
`SHA256SUMS.txt`). Until it's accepted, it installs straight from the repo:

```
scoop install https://raw.githubusercontent.com/Faulik/wireless-battery-tray/main/packaging/scoop/wireless-battery-tray.json
```

## Note on the built-in updater

The app updates its own exe in place. In a package-manager install that works, but winget and
Scoop will keep showing the old version until their own manifest catches up.
