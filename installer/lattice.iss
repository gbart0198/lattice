; Inno Setup script for lattice: builds dist\lattice-setup-<version>.exe.
; Build it with installer\build.ps1, which passes the defines below.
;
; Per-user install, no admin needed, to %LOCALAPPDATA%\Programs\lattice. This is the same
; place install.ps1 uses, so this installer upgrades a script install in place.

#ifndef AppVersion
  #error Pass /DAppVersion=x.y.z (installer\build.ps1 does this)
#endif
#ifndef ExePath
  #define ExePath "..\target\release\lattice.exe"
#endif
#ifndef ConsolePath
  #define ConsolePath "..\target\release\lattice-console.exe"
#endif

[Setup]
; Never change AppId: upgrades and the uninstall entry are keyed on it.
AppId={{6F3A9C1E-2B7D-4E8A-9C5F-1D2E3B4A5C6D}
AppName=lattice
AppVersion={#AppVersion}
AppVerName=lattice {#AppVersion}
AppPublisher=lattice
AppPublisherURL=https://github.com/gbart0198/lattice
AppSupportURL=https://github.com/gbart0198/lattice/issues
AppComments=A tiling window manager for Windows 11
VersionInfoVersion={#AppVersion}
PrivilegesRequired=lowest
DefaultDirName={autopf}\lattice
DisableDirPage=auto
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
WizardStyle=modern
#ifdef IconPath
SetupIconFile={#IconPath}
#endif
UninstallDisplayIcon={app}\lattice.exe
UninstallDisplayName=lattice
OutputBaseFilename=lattice-setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
; [Code] stops lattice cleanly instead, so its hidden windows come back.
CloseApplications=no
; The install folder goes on the user's PATH (for `lattice msg`, `query`, `subscribe`).
ChangesEnvironment=yes

[Tasks]
Name: autostart; Description: "Start lattice when I sign in"; Flags: unchecked

[Files]
Source: "{#ExePath}"; DestDir: "{app}"; Flags: ignoreversion
; The terminal's `lattice` (src/bin/lattice-console.rs): Windows runs .com before .exe.
Source: "{#ConsolePath}"; DestDir: "{app}"; DestName: "lattice.com"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[InstallDelete]
; Left behind by an install.ps1 install; Inno's uninstaller replaces it.
Type: files; Name: "{app}\uninstall.ps1"
; Recreated below if the autostart task is picked, so unticking it on upgrade turns it off.
Type: files; Name: "{userstartup}\lattice.lnk"

[Icons]
Name: "{autoprograms}\lattice"; Filename: "{app}\lattice.exe"; WorkingDir: "{app}"; Comment: "lattice tiling window manager"
Name: "{userstartup}\lattice"; Filename: "{app}\lattice.exe"; WorkingDir: "{app}"; Tasks: autostart

[Run]
Filename: "{app}\lattice.exe"; WorkingDir: "{app}"; Description: "Start lattice now (quit with Win+Shift+E)"; Flags: postinstall nowait skipifsilent
; An update started from lattice (src/update.rs passes /UPDATE) runs silently: start the new one.
Filename: "{app}\lattice.exe"; WorkingDir: "{app}"; Flags: nowait; Check: IsUpdate

[Code]
const
  WM_CLOSE = $0010;
  { Held by the running window manager for its whole life (src/main.rs). }
  LatticeMutex = 'Local\lattice-window-manager';
  LegacyUninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\lattice';

{ lattice's own updater passes /UPDATE; scripted silent installs don't, and get no start. }
function IsUpdate(): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), '/UPDATE') = 0 then
      Result := True;
end;

{ Quit a running lattice the way Win+Shift+E does: closing its control window restores every
  window it hid. Anything still running after 5 s (a hung WM, settings windows) is killed. }
procedure StopLattice();
var
  Wnd: HWND;
  I, Rc: Integer;
begin
  Wnd := FindWindowByWindowName('lattice-control');
  { Builds before the control window only had the bar. }
  if Wnd = 0 then
    Wnd := FindWindowByWindowName('lattice-bar');
  if Wnd <> 0 then
  begin
    PostMessage(Wnd, WM_CLOSE, 0, 0);
    I := 0;
    while CheckForMutexes(LatticeMutex) and (I < 50) do
    begin
      Sleep(100);
      I := I + 1;
    end;
  end;
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM lattice.exe', '', SW_HIDE, ewWaitUntilTerminated, Rc);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopLattice();
  { install.ps1 wrote its own "Installed apps" entry; this installer's entry replaces it. }
  RegDeleteKeyIncludingSubkeys(HKCU, LegacyUninstallKey);
  Result := '';
end;

{ The user PATH with Dir added (Add) or every copy of it taken out. }
function EditedPath(Path, Dir: String; Add: Boolean): String;
var
  Rest, Entry: String;
  P: Integer;
begin
  Result := '';
  Rest := Path;
  while Rest <> '' do
  begin
    P := Pos(';', Rest);
    if P = 0 then
    begin
      Entry := Rest;
      Rest := '';
    end
    else
    begin
      Entry := Copy(Rest, 1, P - 1);
      Delete(Rest, 1, P);
    end;
    if (Entry <> '') and (CompareText(RemoveBackslashUnlessRoot(Entry), RemoveBackslashUnlessRoot(Dir)) <> 0) then
    begin
      if Result <> '' then
        Result := Result + ';';
      Result := Result + Entry;
    end;
  end;
  if Add then
  begin
    if Result <> '' then
      Result := Result + ';';
    Result := Result + Dir;
  end;
end;

procedure SetUserPath(Add: Boolean);
var
  Path: String;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', Path) then
    Path := '';
  Path := EditedPath(Path, ExpandConstant('{app}'), Add);
  { REG_EXPAND_SZ keeps entries like %USERPROFILE%\bin working. }
  RegWriteExpandStringValue(HKCU, 'Environment', 'Path', Path);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    SetUserPath(True);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Rc: Integer;
  ConfigDir: String;
begin
  if CurUninstallStep = usUninstall then
  begin
    StopLattice();
    SetUserPath(False);
    { Show any windows a killed lattice left hidden, while the exe is still here. }
    Exec(ExpandConstant('{app}\lattice.exe'), '--restore', '', SW_HIDE, ewWaitUntilTerminated, Rc);
  end;
  if CurUninstallStep = usPostUninstall then
  begin
    ConfigDir := ExpandConstant('{userappdata}\lattice');
    if DirExists(ConfigDir) and not UninstallSilent() then
      if MsgBox('Also delete your lattice settings and log (' + ConfigDir + ')?',
                mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
        DelTree(ConfigDir, True, True, True);
  end;
end;
