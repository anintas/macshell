#define MyAppName "macshell"
#define MyAppVersion "0.1.0"
#ifndef SourceExe
  #define SourceExe "..\target\release\macshell.exe"
#endif

[Setup]
AppId={{81396C1D-52D7-4A67-A985-8A1A98892414}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher=macshell
DefaultDirName={localappdata}\Programs\macshell
DefaultGroupName=macshell
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=output
OutputBaseFilename=macshell-setup-0.1.0-preview
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\macshell.exe
CloseApplications=yes
CloseApplicationsFilter=macshell.exe

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\macshell"; Filename: "{app}\macshell.exe"
Name: "{autodesktop}\macshell"; Filename: "{app}\macshell.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\macshell.exe"; Description: "{cm:LaunchProgram,macshell}"; Flags: nowait postinstall skipifsilent unchecked
