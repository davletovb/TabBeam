#define HostName "com.pervue.host"

[Setup]
AppId={{D7A1D4E8-774F-4E2B-A745-04B43E51A93C}
AppName=Pervue Companion
AppVersion={#HostVersion}
AppVerName=Pervue Companion {#HostVersion}
AppPublisher=Pervue
DefaultDirName={userpf}\Pervue
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
CloseApplications=yes
CloseApplicationsFilter=pervue-host.exe
RestartApplications=no
UninstallDisplayName=Pervue Companion
VersionInfoVersion={#PackageVersion}.0
#ifdef PervueSignTool
SignTool=pervue
SignedUninstaller=yes
#endif

[Files]
#ifdef PervueSignTool
Source: "{#StageDir}\pervue-host.exe"; DestDir: "{app}"; Flags: ignoreversion signonce
#else
Source: "{#StageDir}\pervue-host.exe"; DestDir: "{app}"; Flags: ignoreversion
#endif
Source: "{#StageDir}\com.pervue.host.json"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\build-info.json"; DestDir: "{app}"; Flags: ignoreversion

[Registry]
; Chrome accepts either registry view. Pervue writes the 64-bit HKCU view on
; supported x64-compatible Windows and removes only its own key on uninstall.
Root: HKCU64; Subkey: "Software\Google\Chrome\NativeMessagingHosts\{#HostName}"; ValueType: string; ValueName: ""; ValueData: "{app}\com.pervue.host.json"; Flags: uninsdeletekey

[Code]
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ExpectedDir: String;
begin
  ExpectedDir := ExpandConstant('{userpf}\Pervue');
  if CompareText(WizardDirValue, ExpectedDir) <> 0 then
    Result := 'Pervue Companion uses a fixed per-user install location. Remove /DIR or /LOADINF overrides and run Setup again.'
  else
    Result := '';
end;
