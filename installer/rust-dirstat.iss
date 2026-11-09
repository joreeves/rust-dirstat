; Inno Setup 6 script for the rust-dirstat installer (setup.exe).
;
; The release workflow builds it with:
;   ISCC.exe /DAppVersion=0.1.0 /DOutputName=rust-dirstat-v0.1.0-windows-x64-setup installer\rust-dirstat.iss
; after `cargo build --release`. The setup file is written to dist\.
;
; The installer:
;   - installs for the current user without an admin prompt (admins can pick "all users");
;   - adds a Start menu shortcut and, optionally, a desktop shortcut;
;   - can add the install folder to PATH so `dirstat scan ...` works in a terminal;
;   - upgrades an existing installation in place (closing a running copy first);
;   - registers an uninstaller under Settings > Apps.
; Silent install: setup.exe /VERYSILENT [/TASKS="addtopath,desktopicon"] [/ALLUSERS]

#ifndef AppVersion
  #error Pass the version, e.g. /DAppVersion=0.1.0
#endif
#ifndef SourceDir
  #define SourceDir "..\target\release"
#endif
#ifndef OutputDir
  #define OutputDir "..\dist"
#endif
#ifndef OutputName
  #define OutputName "rust-dirstat-v" + AppVersion + "-windows-x64-setup"
#endif

#define AppName "rust-dirstat"
#define AppExe "dirstat.exe"
#define AppUrl "https://github.com/joreeves/rust-dirstat"

[Setup]
; Never change AppId: upgrades use it to find the existing installation.
AppId={{DB059964-EBA6-4B8E-8030-F47D5D9EDF5B}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=joreeves
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} setup
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#AppExe}
CloseApplications=yes
RestartApplications=no
ChangesEnvironment=yes

[Tasks]
Name: "addtopath"; Description: "Add to PATH, so ""dirstat"" works in a terminal"; GroupDescription: "Command line:"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; Comment: "Disk usage analyzer"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[Code]
// PATH entry: the user's PATH for a per-user install, the system PATH for an all-users install.

function EnvRoot: Integer;
begin
  if IsAdminInstallMode then
    Result := HKEY_LOCAL_MACHINE
  else
    Result := HKEY_CURRENT_USER;
end;

function EnvKey: String;
begin
  if IsAdminInstallMode then
    Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
  else
    Result := 'Environment';
end;

function SameDir(A, B: String): Boolean;
begin
  Result := CompareText(RemoveBackslashUnlessRoot(Trim(A)), RemoveBackslashUnlessRoot(Trim(B))) = 0;
end;

// Splits a ;-separated PATH value and returns it without the entries equal to Dir (all other
// entries, including empty ones, are kept as they were). Found tells whether any was removed.
function WithoutDir(Path, Dir: String; var Found: Boolean): String;
var
  Rest, Entry: String;
  P: Integer;
  First: Boolean;
begin
  Result := '';
  Found := False;
  First := True;
  Rest := Path + ';';
  while Rest <> '' do
  begin
    P := Pos(';', Rest);
    Entry := Copy(Rest, 1, P - 1);
    Delete(Rest, 1, P);
    if (Entry <> '') and SameDir(Entry, Dir) then
      Found := True
    else
    begin
      if not First then
        Result := Result + ';';
      Result := Result + Entry;
      First := False;
    end;
  end;
end;

procedure AddToPath(Dir: String);
var
  Path: String;
  Found: Boolean;
begin
  if not RegQueryStringValue(EnvRoot, EnvKey, 'Path', Path) then
    Path := '';
  WithoutDir(Path, Dir, Found);
  if Found then
    Exit;
  if (Path <> '') and (Copy(Path, Length(Path), 1) <> ';') then
    Path := Path + ';';
  RegWriteExpandStringValue(EnvRoot, EnvKey, 'Path', Path + Dir);
end;

procedure RemoveFromPath(Dir: String);
var
  Path, Kept: String;
  Found: Boolean;
begin
  if not RegQueryStringValue(EnvRoot, EnvKey, 'Path', Path) then
    Exit;
  Kept := WithoutDir(Path, Dir, Found);
  if Found then
    RegWriteExpandStringValue(EnvRoot, EnvKey, 'Path', Kept);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
  begin
    if WizardIsTaskSelected('addtopath') then
      AddToPath(ExpandConstant('{app}'))
    else
      RemoveFromPath(ExpandConstant('{app}'));
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
