; Compile through bundle-windows.ps1. Keep AppId stable across every release.
#ifndef AppVersion
  #error AppVersion must be supplied by bundle-windows.ps1
#endif
#ifndef NumericVersion
  #error NumericVersion must be supplied by bundle-windows.ps1
#endif
#ifndef SourceDir
  #error SourceDir must be supplied by bundle-windows.ps1
#endif
#ifndef ProjectDir
  #error ProjectDir must be supplied by bundle-windows.ps1
#endif
#ifndef OutputDir
  #error OutputDir must be supplied by bundle-windows.ps1
#endif

[Setup]
AppId={{EE04F97E-B741-43DF-BA62-754508D14491}
AppName=Turntable
AppVersion={#AppVersion}
AppVerName=Turntable {#AppVersion}
VersionInfoVersion={#NumericVersion}
AppPublisher=Khanh Pham
AppPublisherURL=https://github.com/khanhphxm/turntable
AppSupportURL=https://github.com/khanhphxm/turntable/issues
AppUpdatesURL=https://github.com/khanhphxm/turntable/releases
DefaultDirName={localappdata}\Programs\turntable
DefaultGroupName=Turntable
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
UsePreviousAppDir=yes
UsePreviousGroup=yes
UninstallDisplayIcon={app}\turntable.exe
CloseApplications=yes
CloseApplicationsFilter=turntable.exe
RestartApplications=no
LicenseFile={#SourceDir}\LICENSE
SetupIconFile={#ProjectDir}\packaging\icons\turntable.ico
OutputDir={#OutputDir}
OutputBaseFilename=turntable-{#AppVersion}-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Files]
Source: "{#SourceDir}\turntable.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\INSTALL.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Turntable"; Filename: "{app}\turntable.exe"
Name: "{group}\Uninstall Turntable"; Filename: "{uninstallexe}"
