# Windows installation

Run `turntable-<version>-windows-x86_64-setup.exe` for a per-user installation.
It installs under `%LOCALAPPDATA%\Programs\turntable` without administrator
access, adds a Start Menu shortcut and uninstaller, and upgrades an existing
installation in place. Remove it through Windows Settings → Apps or the Start
Menu uninstaller. Installing does not launch the application automatically.

For a portable installation, extract the entire
`turntable-<version>-windows-x86_64.zip` into a folder of your choice, then run
`turntable.exe`. Create your own shortcut to pin it to Start or the taskbar.

Requires Windows 10/11 on x86_64, a working audio output, and an up-to-date GPU
driver supporting Direct3D 11. The executable contains the turntable icon and
GPUI's Windows manifest (PerMonitorV2 DPI awareness, segment heap, and
`asInvoker` privileges). It does not request elevation.

These packages are not Authenticode-signed; SmartScreen may warn about an unknown
publisher. Only download packages from the project's GitHub releases. For a
portable ZIP install, remove the extracted directory and shortcuts to uninstall.

## Window controls

Minimize, maximize/restore and close are at the top bar's right edge, including
on the welcome screen. They use native Windows caption hit-testing; hover the
maximize button for Windows 11 snap layouts. Drag an empty part of the top bar
to move the window, or double-click it to maximize/restore. Resize using the
window's edges or corners.
