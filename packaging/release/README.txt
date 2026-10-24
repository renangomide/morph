Morph {{VERSION}} - convert files from the right-click menu (Windows 11, 64-bit)

INSTALL
1. Extract the whole zip: right-click it > Extract All... > Extract.
   (If Windows warns about files from the internet: right-click the zip > Properties >
   tick "Unblock" > OK, then extract again.)
2. In the extracted folder, double-click Install.cmd.
   - If "Windows protected your PC" appears: click "More info" > "Run anyway".
   - When Windows asks for administrator permission: click Yes. This is needed once,
     to trust Morph's certificate (Morph.cer). It goes to "Trusted People" only,
     not to the trusted root authorities.
3. Wait for "Morph ... is installed." and press a key to close the window.

USE
Right-click photos, PDFs, videos, music or Word/Excel/PowerPoint files in File Explorer
and choose "Morph". Morph is also in the Start menu (settings, drag and drop).
Office documents need Microsoft Office or the free LibreOffice installed.

UNINSTALL
Double-click Uninstall.cmd, or Settings > Apps > Installed apps > Morph > Uninstall.
Uninstall.cmd can also remove the certificate. Files you converted are kept.

WHAT IS IN THIS ZIP
Morph.msix     the app package, signed by "OpenMorph" (self-signed certificate)
Morph.cer      that certificate (public part only)
Install.cmd    trusts Morph.cer and installs Morph.msix
Uninstall.cmd  removes Morph (and, if you want, the certificate)

Morph is free software under the GNU GPL v3 or later. Third-party licenses are in the
"licenses" folder of the installed app.
