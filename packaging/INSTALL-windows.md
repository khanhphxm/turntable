# Windows installation

Extract the entire `turntable-<version>-windows-x86_64.zip` archive into a folder
of your choice, then run `turntable.exe`. No installer or administrator access is
needed. Create a shortcut to pin the application to Start or the taskbar.

Requires Windows 10/11 on x86_64, a working audio output, and an up-to-date GPU
driver supporting Direct3D 11. The executable contains the turntable icon and
GPUI's Windows manifest (PerMonitorV2 DPI awareness, segment heap, and
`asInvoker` privileges). It does not request elevation.

These packages are not Authenticode-signed; SmartScreen may warn about an unknown
publisher. Only download archives from the project's GitHub releases. Remove the
extracted directory and any shortcuts to uninstall the application.
