; The Windows installer (Inno Setup 6), built by the release workflow:
;
;   iscc /DAppVersion=0.2.0 /DBinary=path\to\sisyphus-client.exe /DOutputDir=dist packaging\windows\sisyphus.iss
;
; Installs for the current user only (no admin rights) into
; %LOCALAPPDATA%\Programs\Sisyphus, with a Start menu entry, an optional
; desktop icon, starting at login (checked by default: Sisyphus only
; tracks while it runs) and an uninstaller.
;
; Updates run the new setup silently (see crates/client/src/update.rs):
;   Sisyphus-windows-x86_64-setup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /relaunch=1
; It closes the running copy, keeps the earlier choices and, with
; /relaunch=1, starts the new version in the tray.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef Binary
  #define Binary "..\..\target\x86_64-pc-windows-msvc\release\sisyphus-client.exe"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
; Never change the AppId: Windows recognises updates and the uninstaller by it.
AppId={{7C1D2F3E-5B8A-4E6F-9A0B-2D4C6E8F1A3B}
AppName=Sisyphus
AppVersion={#AppVersion}
AppVerName=Sisyphus {#AppVersion}
AppPublisher=Sisyphus
AppPublisherURL=https://sisyphus.aelx.dev
AppSupportURL=https://github.com/Creative-Difficulty/Sisyphus
VersionInfoVersion={#AppVersion}
PrivilegesRequired=lowest
DefaultDirName={autopf}\Sisyphus
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=Sisyphus-windows-x86_64-setup
SetupIconFile=..\..\crates\client\assets\logo.ico
UninstallDisplayIcon={app}\Sisyphus.exe
UninstallDisplayName=Sisyphus
WizardStyle=modern
Compression=lzma2
SolidCompression=yes
CloseApplications=force
RestartApplications=no

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "autostart"; Description: "Start Sisyphus when I log in (it only tracks while it runs)"
Name: "desktopicon"; Description: "Add a desktop icon"; Flags: unchecked

[Files]
Source: "{#Binary}"; DestDir: "{app}"; DestName: "Sisyphus.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Sisyphus"; Filename: "{app}\Sisyphus.exe"
Name: "{autodesktop}\Sisyphus"; Filename: "{app}\Sisyphus.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Sisyphus"; ValueData: """{app}\Sisyphus.exe"" --background"; Flags: uninsdeletevalue; Tasks: autostart

[Run]
; After a normal install: offer to open it.
Filename: "{app}\Sisyphus.exe"; Description: "Open Sisyphus"; Flags: nowait postinstall skipifsilent
; After an update (/relaunch=1): start it again, in the tray.
Filename: "{app}\Sisyphus.exe"; Parameters: "--background"; Flags: nowait; Check: Relaunch

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/f /im Sisyphus.exe"; Flags: runhidden; RunOnceId: "StopSisyphus"

[Code]
function Relaunch: Boolean;
begin
  Result := ExpandConstant('{param:relaunch|0}') = '1';
end;
