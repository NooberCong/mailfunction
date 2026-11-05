; The Windows installer. Built by scripts\package.ps1, which passes the version and the paths.
; It installs for the current user, so it needs no administrator rights.

[Setup]
AppId={{B7E2C9A4-3F1D-4C68-9A57-2D6E8F10C3B5}
AppName=mailfunction
AppVersion={#Version}
AppVerName=mailfunction {#Version}
AppPublisher=mailfunction
AppPublisherURL=https://github.com/NooberCong/mailfunction
VersionInfoVersion={#Version}
DefaultDirName={autopf}\mailfunction
DisableProgramGroupPage=yes
DisableDirPage=yes
DisableReadyPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
WizardStyle=modern
SetupIconFile=..\crates\app\ui\icon.ico
UninstallDisplayIcon={app}\mailfunction.exe
UninstallDisplayName=mailfunction
CloseApplications=force
RestartApplications=no
Compression=lzma2/max
SolidCompression=yes
OutputDir={#OutputDir}
OutputBaseFilename=mailfunction-setup

[Files]
Source: "{#Exe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\crates\app\ui\fonts\OFL.txt"; DestDir: "{app}"; Flags: ignoreversion

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Icons]
Name: "{autoprograms}\mailfunction"; Filename: "{app}\mailfunction.exe"
Name: "{autodesktop}\mailfunction"; Filename: "{app}\mailfunction.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\mailfunction.exe"; Description: "Open mailfunction"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; The app stays in the notification area with its window closed, and a running exe cannot be
; removed. Only this copy is stopped, by its path. Its webview processes start in this folder and
; outlast it by a moment, and the folder cannot go while they are there.
Filename: "powershell.exe"; WorkingDir: "{app}"; Parameters: "-NoProfile -Command ""Get-Process mailfunction -ErrorAction Ignore | Where-Object Path -eq (Join-Path $PWD mailfunction.exe) | Stop-Process -Force; Start-Sleep 2"""; Flags: runhidden; RunOnceId: "StopApp"

[Registry]
; The app writes these itself. Naming them here is what removes them with it.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "mailfunction"; Flags: dontcreatekey uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\CLSID\{{6F1C2A54-8D3E-4B7A-9C21-5E0D7A3B1F46}"; Flags: dontcreatekey uninsdeletekey

[Code]
// Mail stays behind on uninstalling, as it does with any app, but one identity per account has no
// fixed name to list under [Registry].
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Identities: TArrayOfString;
  I: Integer;
begin
  if (CurUninstallStep = usPostUninstall) and RegGetSubkeyNames(HKCU, 'Software\Classes\AppUserModelId', Identities) then
    for I := 0 to GetArrayLength(Identities) - 1 do
      if (Identities[I] = 'mailfunction') or (Pos('mailfunction.', Identities[I]) = 1) then
        RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Classes\AppUserModelId\' + Identities[I]);
end;
