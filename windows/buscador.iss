; Instalador de buscador para Windows (Inno Setup 6).
; Lo compila .github/workflows/windows.yml:  iscc /DAppVersion=0.2.0 windows\buscador.iss
; Necesita target\release\buscador.exe ya compilado.
;
; Se instala para el usuario (sin pedir permisos de administrador), en
; %LOCALAPPDATA%\Programs\buscador. Así «Buscar actualizaciones» puede ejecutar
; el instalador nuevo en silencio.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
; Identificador fijo: con él Windows reconoce las actualizaciones y el desinstalador.
AppId={{AB9545DC-FC56-4799-922A-F206010A6DA4}
AppName=buscador
AppVersion={#AppVersion}
AppVerName=buscador {#AppVersion}
AppPublisher=airylp
AppPublisherURL=https://github.com/airylp/buscador
AppSupportURL=https://github.com/airylp/buscador/issues
AppUpdatesURL=https://github.com/airylp/buscador/releases
DefaultDirName={autopf}\buscador
DefaultGroupName=buscador
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\target\instalador
OutputBaseFilename=buscador-{#AppVersion}-setup
SetupIconFile=buscador.ico
UninstallDisplayIcon={app}\buscador.exe
UninstallDisplayName=buscador
LicenseFile=..\LICENSE
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Si buscador está abierto, el instalador ofrece cerrarlo (Restart Manager).
CloseApplications=yes
RestartApplications=no
ChangesAssociations=yes
ShowLanguageDialog=auto

[Languages]
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"

[CustomMessages]
es.OpenWith=Abrir con buscador
en.OpenWith=Open with buscador
fr.OpenWith=Ouvrir avec buscador
es.ContextMenu=Añadir «Abrir con buscador» al menú del botón derecho de los archivos
en.ContextMenu=Add “Open with buscador” to the right-click menu of files
fr.ContextMenu=Ajouter « Ouvrir avec buscador » au menu clic droit des fichiers

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "contextmenu"; Description: "{cm:ContextMenu}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\target\release\buscador.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\buscador"; Filename: "{app}\buscador.exe"
Name: "{autodesktop}\buscador"; Filename: "{app}\buscador.exe"; Tasks: desktopicon

[Registry]
; «Abrir con» → buscador, para cualquier tipo de archivo.
Root: HKA; Subkey: "Software\Classes\Applications\buscador.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "buscador"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\buscador.exe\DefaultIcon"; ValueType: string; ValueData: "{app}\buscador.exe,0"
Root: HKA; Subkey: "Software\Classes\Applications\buscador.exe\shell\open\command"; ValueType: string; ValueData: """{app}\buscador.exe"" ""%1"""
Root: HKA; Subkey: "Software\Classes\*\OpenWithList\buscador.exe"; Flags: uninsdeletekey
; Clic derecho en un archivo → «Abrir con buscador» (opcional).
Root: HKA; Subkey: "Software\Classes\*\shell\buscador"; ValueType: string; ValueData: "{cm:OpenWith}"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\*\shell\buscador"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\buscador.exe,0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\*\shell\buscador\command"; ValueType: string; ValueData: """{app}\buscador.exe"" ""%1"""; Tasks: contextmenu

[Run]
; También en las actualizaciones silenciosas (desde la app): vuelve a abrirla.
Filename: "{app}\buscador.exe"; Description: "{cm:LaunchProgram,buscador}"; Flags: nowait postinstall

[Code]
// Guarda el idioma elegido en el instalador como idioma de la app (si aún no
// hay uno elegido), igual que install.sh en macOS/Linux.
procedure CurStepChanged(CurStep: TSetupStep);
var
  Dir, F: String;
  Lines: TArrayOfString;
  I: Integer;
  Has: Boolean;
begin
  if CurStep <> ssPostInstall then
    Exit;
  Dir := ExpandConstant('{userappdata}\buscador');
  F := Dir + '\ajustes.txt';
  Has := False;
  if LoadStringsFromFile(F, Lines) then
    for I := 0 to GetArrayLength(Lines) - 1 do
      if Pos('idioma=', Trim(Lines[I])) = 1 then
        Has := True;
  if not Has then
  begin
    ForceDirectories(Dir);
    SaveStringToFile(F, 'idioma=' + ActiveLanguage + #13#10, True);
  end;
end;
