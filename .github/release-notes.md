## English

Download the file for your computer from the assets below. `SHA256SUMS` lists the checksum of every file.

| Computer                              | File                                                                                  |
| ------------------------------------- | ------------------------------------------------------------------------------------- |
| Windows, Intel or AMD (x64)           | `Pluryprint_<version>_windows_x64.exe`                                                |
| Windows on ARM (Snapdragon, ARM64)    | `Pluryprint_<version>_windows_arm64.exe`                                              |
| Mac with Apple silicon (M1 and later) | `Pluryprint_<version>_apple_silicon.dmg`                                              |
| Mac with an Intel processor           | `Pluryprint_<version>_apple_intel.dmg`                                                |
| Linux, Intel or AMD (x64) (preview)   | `Pluryprint_<version>_linux_x64.AppImage` or `Pluryprint_<version>_linux_x64.deb`     |
| Linux on ARM (ARM64) (preview)        | `Pluryprint_<version>_linux_arm64.AppImage` or `Pluryprint_<version>_linux_arm64.deb` |

Windows on ARM: use the ARM64 file. The x64 file also works there, emulated.

Linux is a preview: it is built and tested in CI, not yet on real hardware. It needs Ubuntu 22.04, Debian 12, Fedora 38 or later (or another distribution with glibc 2.35 and WebKitGTK 2.42 or later).

### Check the download first.

Neither Windows nor macOS can vouch for these files, and no Linux package repository serves them, so check them against `SHA256SUMS` before you open them. Save `SHA256SUMS` next to the file you downloaded, then:

- macOS, in Terminal: `cd ~/Downloads && shasum -a 256 -c SHA256SUMS --ignore-missing` (your file must say `OK`).
- Windows, in PowerShell: `Get-FileHash $HOME\Downloads\Pluryprint_<version>_windows_x64.exe` (or the `arm64` file), and compare the hash with its line in `SHA256SUMS`.
- Linux, in a terminal: `cd ~/Downloads && sha256sum -c SHA256SUMS --ignore-missing` (your file must say `OK`).

### Installing on Linux.

Use the `.deb` on Debian, Ubuntu, Linux Mint and their derivatives; it puts Pluryprint in your applications menu, adds **Open with Pluryprint** and the Dolphin and Nemo actions to the right-click menu, and installs the CUPS tools it prints with:

```
sudo apt install ./Pluryprint_<version>_linux_x64.deb
```

(the `arm64` file on an ARM computer; keep the `./`, or apt looks for a package of that name online). On any other distribution, use the AppImage. Make it executable and run it:

```
chmod +x Pluryprint_<version>_linux_x64.AppImage
./Pluryprint_<version>_linux_x64.AppImage
```

The AppImage needs FUSE 2 (`sudo apt install libfuse2` on Ubuntu 22.04, `libfuse2t64` on Ubuntu 24.04) and the CUPS client tools (`lp`, `lpstat`), which most desktops already have. Both update themselves from inside the app: the AppImage replaces its own file, and the `.deb` asks for your administrator password to install the update.

### macOS will refuse to open it the first time.

The app is ad-hoc signed and not notarized: it carries no Apple Developer ID signature. The DMG mounts normally; drag Pluryprint to Applications. The first time you open it, macOS 15 and later says **"Pluryprint" Not Opened – Apple could not verify "Pluryprint" is free of malware…** (older or other setups may say the app "is damaged"; it is not). Control-click → Open no longer gets past this on macOS 15 and later. Use one of these, once:

- **System Settings → Privacy & Security**, scroll to the message about Pluryprint and choose **Open Anyway**, enter your administrator password, then choose **Open Anyway** again.
- Or clear the quarantine flag in Terminal:

  ```
  xattr -dr com.apple.quarantine /Applications/Pluryprint.app
  ```

Do it only with a copy you downloaded from this release page and checked against `SHA256SUMS`: the quarantine flag is the check that protects you from a tampered download.

### SmartScreen will interrupt the installer.

The installer is not signed with a Microsoft code-signing certificate, so Windows shows the blue "Windows protected your PC" screen. Choose **More info**, then **Run anyway**. If you see a **Security Warning** with **Publisher: Unknown Publisher** instead, choose **Run**. It installs for your user only and asks for no administrator rights.

If Windows says **Smart App Control** blocked the app, there is no Run anyway button: Smart App Control only allows signed or well-known apps. The only way past it is to turn Smart App Control off in Windows Security, and it cannot be turned back on without reinstalling Windows.

Updates are verified against Pluryprint's own signing key either way: an update that was not signed by it is refused.

Pluryprint is free software under the GNU AGPL-3.0. The Windows installers include SumatraPDF (GPL-3.0); the source of both is attached to this release. The Linux AppImages also bundle system libraries under the LGPL (WebKitGTK, GTK, GLib and others); `THIRD-PARTY-NOTICES.md` inside the AppImage says where their source is.

## Español

Descarga el archivo de tu equipo desde la lista de abajo. `SHA256SUMS` recoge la suma de comprobación de cada archivo.

| Equipo                                    | Archivo                                                                              |
| ----------------------------------------- | ------------------------------------------------------------------------------------ |
| Windows, Intel o AMD (x64)                | `Pluryprint_<versión>_windows_x64.exe`                                               |
| Windows en ARM (Snapdragon, ARM64)        | `Pluryprint_<versión>_windows_arm64.exe`                                             |
| Mac con Apple silicon (M1 y posteriores)  | `Pluryprint_<versión>_apple_silicon.dmg`                                             |
| Mac con procesador Intel                  | `Pluryprint_<versión>_apple_intel.dmg`                                               |
| Linux, Intel o AMD (x64) (versión previa) | `Pluryprint_<versión>_linux_x64.AppImage` o `Pluryprint_<versión>_linux_x64.deb`     |
| Linux en ARM (ARM64) (versión previa)     | `Pluryprint_<versión>_linux_arm64.AppImage` o `Pluryprint_<versión>_linux_arm64.deb` |

Windows en ARM: usa el archivo ARM64. El x64 también funciona, emulado.

Linux es una versión previa: se compila y se prueba en CI, pero aún no en equipos reales. Necesita Ubuntu 22.04, Debian 12, Fedora 38 o posteriores (u otra distribución con glibc 2.35 y WebKitGTK 2.42 o posteriores).

### Comprueba antes la descarga.

Ni Windows ni macOS pueden avalar estos archivos, y ningún repositorio de paquetes de Linux los distribuye, así que compruébalos con `SHA256SUMS` antes de abrirlos. Guarda `SHA256SUMS` junto al archivo descargado y después:

- macOS, en Terminal: `cd ~/Downloads && shasum -a 256 -c SHA256SUMS --ignore-missing` (tu archivo debe decir `OK`).
- Windows, en PowerShell: `Get-FileHash $HOME\Downloads\Pluryprint_<versión>_windows_x64.exe` (o el archivo `arm64`) y compara el resultado con su línea de `SHA256SUMS`.
- Linux, en una terminal: `cd ~/Downloads && sha256sum -c SHA256SUMS --ignore-missing` (tu archivo debe decir `OK`).

### Instalación en Linux.

Usa el `.deb` en Debian, Ubuntu, Linux Mint y sus derivadas: añade Pluryprint al menú de aplicaciones, **Abrir con Pluryprint** y las acciones de Dolphin y Nemo al menú del clic derecho, e instala las herramientas de CUPS con las que imprime:

```
sudo apt install ./Pluryprint_<versión>_linux_x64.deb
```

(el archivo `arm64` en un equipo ARM; no quites el `./`, o apt buscará en internet un paquete con ese nombre). En cualquier otra distribución, usa la AppImage. Dale permiso de ejecución y ábrela:

```
chmod +x Pluryprint_<versión>_linux_x64.AppImage
./Pluryprint_<versión>_linux_x64.AppImage
```

La AppImage necesita FUSE 2 (`sudo apt install libfuse2` en Ubuntu 22.04, `libfuse2t64` en Ubuntu 24.04) y las herramientas de cliente de CUPS (`lp`, `lpstat`), que la mayoría de escritorios ya traen. Las dos se actualizan desde la propia app: la AppImage sustituye su propio archivo y el `.deb` pide tu contraseña de administrador para instalar la actualización.

### macOS no la abrirá la primera vez.

La app lleva una firma ad hoc y no está notarizada: no tiene firma de Apple Developer ID. El DMG se monta con normalidad; arrastra Pluryprint a Aplicaciones. La primera vez que la abras, macOS 15 y posteriores dice **No se ha abierto «Pluryprint» – Apple no ha podido verificar que «Pluryprint» no contenga software malicioso…** (otras configuraciones, o versiones anteriores, pueden decir que la app «está dañada»; no lo está). En macOS 15 y posteriores, Control-clic → Abrir ya no permite saltarse este aviso. Usa una de estas dos vías, una sola vez:

- **Ajustes del Sistema → Privacidad y seguridad**, baja hasta el mensaje sobre Pluryprint y elige **Abrir igualmente**, escribe tu contraseña de administrador y elige **Abrir igualmente** otra vez.
- O quita la marca de cuarentena en Terminal:

  ```
  xattr -dr com.apple.quarantine /Applications/Pluryprint.app
  ```

Hazlo solo con una copia descargada de esta página y comprobada con `SHA256SUMS`: la marca de cuarentena es la comprobación que te protege de una descarga manipulada.

### SmartScreen interrumpirá el instalador.

El instalador no está firmado con un certificado de firma de código de Microsoft, así que Windows muestra la pantalla azul "Windows protegió su PC". Elige **Más información** y después **Ejecutar de todas formas**. Si en su lugar ves una **Advertencia de seguridad** con **Editor: Editor desconocido**, elige **Ejecutar**. Se instala solo para tu usuario y no pide permisos de administrador.

Si Windows dice que **Control inteligente de aplicaciones** bloqueó la app, no hay botón Ejecutar de todas formas: solo admite apps firmadas o conocidas. La única salida es desactivarlo en Seguridad de Windows, y no se puede volver a activar sin reinstalar Windows.

Las actualizaciones se verifican siempre contra la clave de firma de Pluryprint: una actualización que no venga firmada por ella se rechaza.

Pluryprint es software libre bajo la GNU AGPL-3.0. Los instaladores de Windows incluyen SumatraPDF (GPL-3.0); el código fuente de ambos va adjunto a esta versión. Las AppImage de Linux incluyen además bibliotecas del sistema bajo la LGPL (WebKitGTK, GTK, GLib y otras); el archivo `THIRD-PARTY-NOTICES.md` de la AppImage indica dónde está su código fuente.
