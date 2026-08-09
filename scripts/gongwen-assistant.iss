; Gongwen Assistant Windows installer (Inno Setup 6)
; Build: package-installer.ps1 or ISCC.exe /DMyAppVersion=... /DMySourceDir=... /DMyOutputDir=...

#ifndef MyAppVersion
  #define MyAppVersion "0.0.0"
#endif
#ifndef MySourceDir
  #define MySourceDir "..\dist\win-x64-full"
#endif
#ifndef MyOutputDir
  #define MyOutputDir "..\dist"
#endif
#ifndef MyIconPath
  #define MyIconPath "..\assets\app-icon\app-icon.ico"
#endif
#ifndef MyLanguageFile
  #define MyLanguageFile "ChineseSimplified.isl"
#endif

#define MyAppName "公文助手"
#define MyAppExeName "gongwen-assistant.exe"
#define MyAppId "{06988153-58ff-4ce5-b78d-1ffd98a796ce}"

[Setup]
AppId={{#MyAppId}}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher=Gongwen
AppPublisherURL=https://github.com/billowsand/gongwen
AppSupportURL=https://github.com/billowsand/gongwen/issues
DefaultDirName={localappdata}\Programs\GongwenAssistant
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
SourceDir="{#MySourceDir}"
OutputDir="{#MyOutputDir}"
OutputBaseFilename=gongwen-assistant-{#MyAppVersion}-win-x64-setup
SetupIconFile="{#MyIconPath}"
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64
ArchitecturesInstallIn64BitMode=x64
CloseApplications=yes
CloseApplicationsFilter={#MyAppExeName}
RestartApplications=no

[Languages]
Name: "chinesesimplified"; MessagesFile: "{#MyLanguageFile}"

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务："; Flags: unchecked

[Files]
Source: "*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[InstallDelete]
Type: filesandordirs; Name: "{app}\runtime"
Type: filesandordirs; Name: "{app}\licenses"

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "启动 {#MyAppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: filesandordirs; Name: "{app}"
