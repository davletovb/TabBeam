#define HostName "com.tabbeam.host"

[Setup]
AppId={{E302F901-5CE0-4244-963F-875FD08058B1}
AppName=TabBeam Companion
AppVersion={#HostVersion}
AppVerName=TabBeam Companion {#HostVersion}
AppPublisher=TabBeam
DefaultDirName={userpf}\TabBeam
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=TabBeam-{#HostVersion}-{#SourceCommitShort}-windows-x64
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
SetupLogging=yes
UsePreviousAppDir=no
CloseApplications=yes
CloseApplicationsFilter=tabbeam-host.exe
RestartApplications=no
UninstallDisplayName=TabBeam Companion
VersionInfoVersion={#PackageVersion}.0
#ifdef TabBeamSignTool
SignTool=tabbeam
SignedUninstaller=yes
#endif

[Files]
#ifdef TabBeamSignTool
Source: "{#StageDir}\tabbeam-host.exe"; DestDir: "{app}"; Flags: ignoreversion signonce
#else
Source: "{#StageDir}\tabbeam-host.exe"; DestDir: "{app}"; Flags: ignoreversion
#endif
Source: "{#StageDir}\com.tabbeam.host.json"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\build-info.json"; DestDir: "{app}"; Flags: ignoreversion

[Registry]
; Chrome accepts either registry view. TabBeam writes the 64-bit HKCU view on
; supported x64-compatible Windows and removes only its own key on uninstall.
Root: HKCU64; Subkey: "Software\Google\Chrome\NativeMessagingHosts\{#HostName}"; ValueType: string; ValueName: ""; ValueData: "{app}\com.tabbeam.host.json"; Flags: uninsdeletekey

[Code]
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ExpectedDir: String;
begin
  ExpectedDir := ExpandConstant('{userpf}\TabBeam');
  if CompareText(WizardDirValue, ExpectedDir) <> 0 then
    Result := 'TabBeam Companion uses a fixed per-user install location. Remove /DIR or /LOADINF overrides and run Setup again.'
  else
    Result := '';
end;
