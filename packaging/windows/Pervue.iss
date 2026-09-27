#define HostName "com.pervue.host"

[Setup]
AppId={{D7A1D4E8-774F-4E2B-A745-04B43E51A93C}
AppName=Pervue Companion
AppVersion={#HostVersion}
AppVerName=Pervue Companion {#HostVersion}
AppPublisher=Pervue
DefaultDirName={localappdata}\Pervue
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=Pervue-{#HostVersion}-{#SourceCommitShort}-windows-x64
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
SetupLogging=yes
UsePreviousAppDir=no
CloseApplications=no
RestartApplications=no
UninstallDisplayName=Pervue Companion
UninstallFilesDir={app}
VersionInfoVersion={#PackageVersion}.0

[Files]
Source: "{#StageDir}\pervue-host.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\com.pervue.host.json"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\build-info.json"; DestDir: "{app}"; Flags: ignoreversion

[Registry]
; Chrome accepts either registry view. Pervue writes the 64-bit HKCU view on
; supported x64-compatible Windows and removes only its own key on uninstall.
Root: HKCU64; Subkey: "Software\Google\Chrome\NativeMessagingHosts\{#HostName}"; ValueType: string; ValueName: ""; ValueData: "{app}\com.pervue.host.json"; Flags: uninsdeletekey

[UninstallDelete]
Type: files; Name: "{app}\com.pervue.host.json"
Type: files; Name: "{app}\build-info.json"
Type: filesandordirs; Name: "{app}"
