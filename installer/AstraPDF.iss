#ifndef PayloadDir
  #error PayloadDir is required
#endif
#ifndef OutputDir
  #error OutputDir is required
#endif
#ifndef AppVersion
  #error AppVersion is required
#endif
#ifndef VersionNumbers
  #error VersionNumbers is required
#endif

#ifdef TestIdentity
  #define ProductName "Astra PDF Installer Test"
  #define ProductId "AstraPDF.InstallerTest"
  #define ProductGuid "8B24310F-20F8-494B-B9C8-3E40C37A5A5D"
  #define ExeName "AstraPDF-InstallerTest.exe"
  #define SettingsName "Astra%20PDF%20Installer%20Test"
  #define OutputName "AstraPDF-" + AppVersion + "-setup-test-x64"
#else
  #define ProductName "Astra PDF"
  #define ProductId "AstraPDF"
  #define ProductGuid "4C5F0E6B-825B-4A27-8A96-3D71CD9DBF62"
  #define ExeName "AstraPDF.exe"
  #define SettingsName "Astra%20PDF"
  #define OutputName "AstraPDF-" + AppVersion + "-setup-x64"
#endif
#define DocumentId ProductId + ".Document"
#define DefaultSettings "ms-settings:defaultapps?registeredAppUser=" + SettingsName

[Setup]
AppId={{{#ProductGuid}}
AppName={#ProductName}
AppVersion={#AppVersion}
AppVerName={#ProductName} {#AppVersion}
AppPublisher=Astra PDF contributors
AppPublisherURL=https://github.com/BrianIS8090/astra-pdf
AppSupportURL=https://github.com/BrianIS8090/astra-pdf/issues
AppUpdatesURL=https://github.com/BrianIS8090/astra-pdf/releases
VersionInfoVersion={#VersionNumbers}
VersionInfoDescription=Установка {#ProductName}
DefaultDirName={localappdata}\Programs\{#ProductName}
DefaultGroupName={#ProductName}
PrivilegesRequired=lowest
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
MinVersion=10.0.22000
DisableProgramGroupPage=yes
DisableWelcomePage=no
DisableDirPage=auto
UsePreviousAppDir=yes
UsePreviousTasks=yes
UninstallDisplayName={#ProductName}
UninstallDisplayIcon={app}\{#ExeName}
SetupIconFile=..\assets\app.ico
WizardStyle=modern
WizardSizePercent=110
Compression=lzma2
SolidCompression=yes
ChangesAssociations=yes
CloseApplications=yes
CloseApplicationsFilter=*.exe,*.dll
RestartApplications=no
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}

[Languages]
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Messages]
WelcomeLabel2=Программа будет установлена для вашей учётной записи Windows.%n%nУстановщик добавит Astra PDF в список приложений для открытия PDF. На последнем шаге можно перейти к выбору приложения по умолчанию.%n%nПеред обновлением закройте окна Astra PDF.
FinishedLabel=Программа установлена. Оставьте флажок ниже, чтобы выбрать её для PDF.%n%nВ настройках Windows найдите .pdf. Если открыт общий список, введите .pdf в верхнем поле и нажмите Enter.%n%nНажмите текущее приложение, выберите Astra PDF и «Задать по умолчанию».

[Tasks]
Name: "desktopicon"; Description: "Создать ярлык на рабочем столе"; Flags: unchecked

[Files]
Source: "{#PayloadDir}\AstraPDF.exe"; DestDir: "{app}"; DestName: "{#ExeName}"; Flags: ignoreversion
Source: "{#PayloadDir}\*"; DestDir: "{app}"; Excludes: "AstraPDF.exe,SHA256.json"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "INSTALLATION.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\{#ProductName}\{#ProductName}"; Filename: "{app}\{#ExeName}"; WorkingDir: "{app}"
Name: "{userprograms}\{#ProductName}\Выбрать для PDF по умолчанию"; Filename: "{#DefaultSettings}"; IconFilename: "{app}\{#ExeName}"
Name: "{userdesktop}\{#ProductName}"; Filename: "{app}\{#ExeName}"; WorkingDir: "{app}"; Tasks: desktopicon

[Registry]
; Только собственная регистрация; выбор пользователя в Windows не перезаписывается.
Root: HKCU; Subkey: "Software\Classes\{#DocumentId}"; ValueType: string; ValueData: "PDF-документ {#ProductName}"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\{#DocumentId}"; ValueName: "FriendlyTypeName"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\{#DocumentId}\Application"; ValueType: string; ValueName: "ApplicationName"; ValueData: "{#ProductName}"
Root: HKCU; Subkey: "Software\Classes\{#DocumentId}\Application"; ValueType: string; ValueName: "ApplicationCompany"; ValueData: "Astra PDF contributors"
Root: HKCU; Subkey: "Software\Classes\{#DocumentId}\Application"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "Просмотр PDF, слои, чертежи и печать"
Root: HKCU; Subkey: "Software\Classes\{#DocumentId}\Application"; ValueType: string; ValueName: "ApplicationIcon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKCU; Subkey: "Software\Classes\{#DocumentId}\DefaultIcon"; ValueType: string; ValueData: """{app}\{#ExeName}"",0"
Root: HKCU; Subkey: "Software\Classes\{#DocumentId}\shell\open\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKCU; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: string; ValueName: "{#DocumentId}"; ValueData: ""; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\Applications\{#ExeName}"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "{#ProductName}"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Applications\{#ExeName}\SupportedTypes"; ValueType: string; ValueName: ".pdf"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\{#ExeName}\DefaultIcon"; ValueType: string; ValueData: """{app}\{#ExeName}"",0"
Root: HKCU; Subkey: "Software\Classes\Applications\{#ExeName}\shell\open\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKCU; Subkey: "Software\{#ProductId}"; Flags: uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\{#ProductId}\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "{#ProductName}"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\{#ProductId}\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "Просмотр PDF, миниатюры страниц, слои, чертежи и печать"
Root: HKCU; Subkey: "Software\{#ProductId}\Capabilities"; ValueType: string; ValueName: "ApplicationIcon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKCU; Subkey: "Software\{#ProductId}\Capabilities\FileAssociations"; ValueType: string; ValueName: ".pdf"; ValueData: "{#DocumentId}"
Root: HKCU; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "{#ProductName}"; ValueData: "Software\{#ProductId}\Capabilities"; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\{#ExeName}"; ValueType: string; ValueData: "{app}\{#ExeName}"; Flags: uninsdeletekey

[Run]
Filename: "{#DefaultSettings}"; Description: "Выбрать Astra PDF для PDF по умолчанию"; Flags: postinstall shellexec nowait skipifsilent
Filename: "{app}\{#ExeName}"; Description: "Запустить Astra PDF"; Flags: postinstall nowait skipifsilent unchecked
