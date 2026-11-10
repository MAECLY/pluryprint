# Linux file-manager integration

The Linux counterpart of the Explorer menu (`shell-ext-win/`) and the macOS
Services (`Info.plist`). There is no extension to load: each file manager reads
a static file, and every action runs the `pluryprint` binary.

| File                                    | Installed at (.deb)                                                                                                                               | What it does                                                                                                                                                                           |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `pluryprint.desktop.hbs`                | `bundle.linux.deb.desktopTemplate`, rendered by the bundler to `/usr/share/applications/Pluryprint.desktop` (the AppImage carries the same entry) | The app's launcher. `MimeType=` lists every printable format, so Pluryprint shows under "Open with" everywhere; `Exec=pluryprint %F` opens the Print Job screen with the files loaded. |
| `kio-servicemenu.desktop`               | `/usr/share/kio/servicemenus/pluryprint.desktop`                                                                                                  | Dolphin: a "Pluryprint" submenu with Print now, Configure print job… and Add to print queue. Read by KDE Frameworks 6 and 5.85+.                                                       |
| `nemo/pluryprint-print-now.nemo_action` | `/usr/share/nemo/actions/pluryprint-print-now.nemo_action`                                                                                        | Nemo: Print now with Pluryprint.                                                                                                                                                       |
| `nemo/pluryprint-configure.nemo_action` | `/usr/share/nemo/actions/pluryprint-configure.nemo_action`                                                                                        | Nemo: Configure print job in Pluryprint…                                                                                                                                               |
| `nemo/pluryprint-queue.nemo_action`     | `/usr/share/nemo/actions/pluryprint-queue.nemo_action`                                                                                            | Nemo: Add to the Pluryprint print queue.                                                                                                                                               |

All files are plain data (mode 0644); none needs a maintainer script. Dolphin
and Nemo pick up a new or removed menu on their next start, and the desktop
database is refreshed by the `.deb`'s standard triggers.

## The command line the menus run

```
pluryprint --invoke=context-menu --action=<print-default|configure|queue> -- %F
```

The paths after `--` are the selection. The Nemo actions add `Quote=double`:
Nemo builds the command as one string and splits it like a shell, and Nemo 5.2
to 5.4 (Ubuntu 22.04, Mint 21.0) leave the spaces in unquoted paths as they are. This transport exists on Linux only
(`cli::ARGV_SELECTION`): the Windows and macOS integrations keep passing a
handoff file with `--files-from`, and refuse paths on the command line. A launch
while the app is running is forwarded to the open window through the
single-instance plugin, which needs a D-Bus session bus.

`print-default` prints with the default profile, `configure` opens the Print Job
screen, and `queue` adds the files to the queue paused, the same three actions
as the Explorer submenu. Folders qualify and are walked one level deep for
`print-default` and `queue`; `configure` asks, as a drop does.

## Not covered here

- Nautilus (GNOME Files) has no static action format; it only gets "Open with".
- The AppImage installs nothing outside itself, so it gets no file-manager
  entries unless an AppImage integrator (AppImageLauncher, Gear Lever) registers
  its launcher.
- Thunar's custom actions live in each user's `uca.xml`; a user can add one
  with the command line above.

`shell_menu::linux_files` (in `src-tauri/src/shell_menu.rs`) pins these files
against the engine's format list, the i18n catalogs' `shellMenu` labels and the
CLI parser, so a new format or a renamed label fails a test instead of a menu.
