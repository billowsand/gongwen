; Gongwen Assistant Windows installer (Inno Setup 6)
; Build: package-installer.ps1 (完整包) 或 package-upgrade.ps1 (增量升级包)。
; 两种模式共用同一个 AppId，共用同一份卸载日志（unins000.dat），所以升级包装完
; 仍由同一个卸载器清理，卸载条目也只有一条。
;
; 增量升级包（MyUpgradeMode=1）只携带与基线不同的文件：
;   - 不整目录删除 {app}\runtime 与 {app}\licenses（否则没随包携带的字体会被删掉），
;     只按基线清单删除真正消失的文件（由 package-upgrade.ps1 生成 include 片段）；
;   - 不重建开始菜单 / 桌面快捷方式，也不显示任务页；
;   - 若目标目录里没有已安装的程序，直接报错要求先装完整包。

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
; 0 = 完整包，1 = 增量升级包（ISPP 的 #if 只对字符串做比较，所以带引号）
#ifndef MyUpgradeMode
  #define MyUpgradeMode "0"
#endif
#if MyUpgradeMode == "1"
  #define MyPackageKind "-upgrade-setup"
#else
  #define MyPackageKind "-setup"
#endif
; 升级包安装前删除的过时文件清单，由 package-upgrade.ps1 生成。
; ISPP 的 #include 把反斜杠当转义符，所以这里的路径用正斜杠。
#ifndef MyStaleListInclude
  #define MyStaleListInclude "../dist/win-x64-stale.iss"
#endif

#define MyAppName "公文助手"
#define MyAppExeName "gongwen-assistant.exe"
; AppId 用 #ifndef 包起来只是为了能在沙箱里换 GUID 做安装测试，
; 正式打包（package-installer.ps1）不传这个宏，取值与历史版本保持一致。
#ifndef MyAppId
  #define MyAppId "{06988153-58ff-4ce5-b78d-1ffd98a796ce}"
#endif

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
OutputBaseFilename=gongwen-assistant-{#MyAppVersion}-win-x64{#MyPackageKind}
SetupIconFile="{#MyIconPath}"
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
; 本包是 x64 二进制，x64compatible 表示 x64 系统与 ARM64 上的 x64 模拟都可安装；
; 旧写法 x64 在 Inno Setup 6.5+ 已废弃（会被替换成 x64os 并告警）。
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes
CloseApplicationsFilter={#MyAppExeName}
RestartApplications=no
#if MyUpgradeMode == "1"
; 升级包覆盖的是同一份安装，沿用上一版的目录与显示名，只更新版本号
UsePreviousAppDir=yes
UpdateUninstallLogAppName=no
#endif

[Languages]
Name: "chinesesimplified"; MessagesFile: "{#MyLanguageFile}"

#if MyUpgradeMode == "0"
[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务："; Flags: unchecked
#endif

[Files]
Source: "*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[InstallDelete]
#if MyUpgradeMode == "1"
#include MyStaleListInclude
#else
Type: filesandordirs; Name: "{app}\runtime"
Type: filesandordirs; Name: "{app}\licenses"
#endif

#if MyUpgradeMode == "0"
[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"; Tasks: desktopicon
#endif

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "启动 {#MyAppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: filesandordirs; Name: "{app}"

#if MyUpgradeMode == "1"
[Code]
{ 升级包必须装在已有安装之上，否则没携带的资源（字体等）会缺失。
  检查放在 PrepareToInstall：此时安装目录已经定下来，InitializeSetup 里还没有。 }
function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  if not FileExists(ExpandConstant('{app}\' + '{#MyAppExeName}')) then
    Result := '未在目标目录找到已安装的 {#MyAppName}：' + ExpandConstant('{app}') + #13#10 + #13#10 +
      '本增量升级包只包含与旧版本不同的文件，不能用于全新安装。' +
      '请先安装完整安装包，再运行本升级包。';
end;
#endif
