; Windows installer for Layr (Inno Setup 6).
;
;   iscc /DAppVersion=1.0.0 /DSourceExe=target\release\layr.exe /DOutputDir=dist packaging\windows\layr.iss
;
; Installs for the current user by default (no admin prompt), or for everyone when the user
; picks it; adds a Start menu entry, an optional desktop shortcut and an uninstaller.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\..\target\release\layr.exe"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
AppId={{197D54CD-FF84-49D0-8C7B-B83B03F586D6}
AppName=Layr
AppVersion={#AppVersion}
AppVerName=Layr {#AppVersion}
AppPublisher=Layr
AppPublisherURL=https://github.com/adamhlt/Layr
AppSupportURL=https://github.com/adamhlt/Layr/issues
DefaultDirName={autopf}\Layr
DefaultGroupName=Layr
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=layr-windows-x64-setup
SetupIconFile=..\..\assets\icon\icon.ico
LicenseFile=..\..\LICENSE
UninstallDisplayIcon={app}\layr.exe
UninstallDisplayName=Layr
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "french"; MessagesFile: "compiler:Languages\French.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "layr.exe"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\..\assets\Inter-LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\assets\IBMPlexSans-OFL.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\vendor\rune-basalt\assets\fonts\SpaceGrotesk-OFL.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\vendor\rune-basalt\assets\fonts\JetBrainsMono-OFL.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Layr"; Filename: "{app}\layr.exe"
Name: "{autodesktop}\Layr"; Filename: "{app}\layr.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\layr.exe"; Description: "{cm:LaunchProgram,Layr}"; Flags: nowait postinstall skipifsilent
