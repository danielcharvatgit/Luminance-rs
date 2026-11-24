; Inno Setup script for the Windows installer.
;
; build.sh stages the application in a "Luminance" folder next to a copy of
; this file (dist/windows/) and compiles it with ISCC (Inno Setup 6 or 7) under
; wine. It can also be compiled on Windows: open dist/windows/luminance.iss
; in the Inno Setup Compiler, or run
;     ISCC.exe luminance.iss
;
; AppVersion is the semantic version (https://semver.org/), e.g. 2.1.0 or
; 2.2.0-rc.1. AppNumericVersion is its MAJOR.MINOR.PATCH.0 form, which the
; installer's Windows version resource needs. build.sh fills in both.

#ifndef AppVersion
  #define AppVersion "3.0.0"
#endif
#ifndef AppNumericVersion
  #define AppNumericVersion "3.0.0.0"
#endif
#define AppName "Luminance"
#define AppExe "luminance-rs.exe"

; The copy of this file in packaging/windows/ has no application next to it:
; compiling it there would produce an installer that installs nothing.
#if !FileExists(AddBackslash(SourcePath) + "Luminance\bin\" + AppExe)
  #error Luminance\bin\luminance-rs.exe not found next to this script. Run ./build.sh exe and compile dist/windows/luminance.iss instead.
#endif

[Setup]
AppId={{78947CB8-8994-4FDE-AFE2-B02D53EEE82A}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
VersionInfoVersion={#AppNumericVersion}
VersionInfoProductTextVersion={#AppVersion}
AppPublisher=Luminance-rs contributors
AppPublisherURL=https://github.com/sidevesh/Luminance
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
; Per-user install without administrator rights is offered as well.
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile=Luminance\LICENSE.txt
SetupIconFile=Luminance\luminance.ico
UninstallDisplayIcon={app}\bin\{#AppExe}
OutputDir=.
OutputBaseFilename=Luminance-{#AppVersion}-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ChangesEnvironment=yes
; Close a running Luminance before replacing its files.
CloseApplications=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "addtopath"; Description: "Add the command line tool (luminance-rs-cli) to PATH"; Flags: unchecked

[Files]
Source: "Luminance\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\bin\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\bin\{#AppExe}"; Tasks: desktopicon

[Registry]
; PATH entry for the current user only, removed again by the uninstaller.
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; \
    ValueData: "{olddata};{app}\bin"; Tasks: addtopath; Check: NeedsAddPath(ExpandConstant('{app}\bin'))

[Run]
Filename: "{app}\bin\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[Code]
function NeedsAddPath(Dir: string): Boolean;
var
  Path: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path) then
    Path := '';
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') = 0;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Path, Dir: string;
  P: Integer;
begin
  if CurUninstallStep <> usPostUninstall then
    exit;
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path) then
    exit;
  Dir := ExpandConstant('{app}\bin');
  // Match whole entries only (also the first one, which has no leading ';'),
  // so e.g. "...\Luminance\bin2" is left alone.
  Path := ';' + Path + ';';
  P := Pos(';' + Uppercase(Dir) + ';', Uppercase(Path));
  if P > 0 then
  begin
    Delete(Path, P, Length(Dir) + 1);
    Path := Copy(Path, 2, Length(Path) - 2);
    RegWriteExpandStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path);
  end;
end;
