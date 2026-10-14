; The DisplayCraft installer, compiled with Inno Setup by make-app.ps1,
; which passes the version, the assembled folder and where the installer
; goes:
;   ISCC /DAppVersion=0.1.0 /DAppDir=...\DisplayCraft /DOutDir=...\dist DisplayCraft.iss
;
; Installs for the current user without administrator rights (in
; %LOCALAPPDATA%\Programs), with a Start menu entry and an uninstaller; an
; administrator can choose to install for everyone instead.

#ifndef AppVersion
  #error AppVersion isn't defined (pass /DAppVersion=...)
#endif
#ifndef AppDir
  #error AppDir isn't defined (pass /DAppDir=...)
#endif
#ifndef OutDir
  #error OutDir isn't defined (pass /DOutDir=...)
#endif

[Setup]
; Identifies DisplayCraft to Windows across versions, so a new version
; replaces the old one; never change it
AppId={{28793EDD-E735-4F29-92F3-82CC2F55CA59}
AppName=DisplayCraft
AppVersion={#AppVersion}
AppVerName=DisplayCraft {#AppVersion}
AppPublisher=Jan Brittenson
AppPublisherURL=https://github.com/bson/displaycraft
AppSupportURL=https://github.com/bson/displaycraft
DefaultDirName={autopf}\DisplayCraft
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutDir}
OutputBaseFilename=DisplayCraft-{#AppVersion}-windows-x64-setup
SetupIconFile={#AppDir}\displaycraft.ico
UninstallDisplayIcon={app}\displaycraft.ico
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Files]
Source: "{#AppDir}\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion

[Icons]
Name: "{autoprograms}\DisplayCraft"; Filename: "{app}\displaycraft.exe"; IconFilename: "{app}\displaycraft.ico"

[Run]
Filename: "{app}\displaycraft.exe"; Description: "Start DisplayCraft"; Flags: nowait postinstall skipifsilent
