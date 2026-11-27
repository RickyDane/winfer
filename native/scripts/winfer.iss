; Inno Setup script for the native Windows installer, built by bundle-windows.sh:
;   iscc /DAppVersion=… /DSourceExe=<portable .exe> /DArch=<x64compatible|arm64> /DOutName=<file name> winfer.iss
; Installs per user (no admin prompt) into %LOCALAPPDATA%\Programs\winfer. "Install and restart"
; updates it in place there, asking for administrator permission first (updates.rs).
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef Arch
  #define Arch "x64compatible"
#endif
#ifndef OutName
  #define OutName "winfer-native_0.0.0_windows-x86_64-setup"
#endif

[Setup]
AppId={{6F1B7C52-2D4E-4B8A-9C3A-5E0D8A7B1F64}
AppName=winfer
AppVersion={#AppVersion}
AppPublisher=winfer
DefaultDirName={autopf}\winfer
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed={#Arch}
ArchitecturesInstallIn64BitMode={#Arch}
SetupIconFile=..\..\src-tauri\icons\icon.ico
UninstallDisplayIcon={app}\winfer.exe
OutputDir=..\target\bundle
OutputBaseFilename={#OutName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "winfer.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\winfer"; Filename: "{app}\winfer.exe"
Name: "{autodesktop}\winfer"; Filename: "{app}\winfer.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\winfer.exe"; Description: "Start winfer"; Flags: nowait postinstall skipifsilent

; Updates leave a winfer.exe.old and a staging folder beside the .exe.
[UninstallDelete]
Type: filesandordirs; Name: "{app}"
