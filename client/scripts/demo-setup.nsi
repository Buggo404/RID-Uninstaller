; A throwaway installer for checking install tracing by hand: it does what ordinary setups do
; (files, a data folder named differently from the program, registry, uninstall entry, shortcut).
;   makensis demo-setup.nsi   ->   demo-setup.exe   (run with /S for a silent install)
Unicode true
Name "Rid Trace Demo"
OutFile "demo-setup.exe"
InstallDir "$PROGRAMFILES64\RidTraceDemo"
RequestExecutionLevel admin

Section
  SetShellVarContext all
  SetOutPath "$INSTDIR"
  FileOpen $0 "$INSTDIR\demo.txt" w
  FileWrite $0 "demo"
  FileClose $0
  CreateDirectory "$INSTDIR\data"
  FileOpen $0 "$INSTDIR\data\part.tmp" w
  FileWrite $0 "x"
  FileClose $0
  Rename "$INSTDIR\data\part.tmp" "$INSTDIR\data\part.bin"
  CreateDirectory "$APPDATA\OddlyNamedDemoData"
  FileOpen $0 "$APPDATA\OddlyNamedDemoData\settings.ini" w
  FileWrite $0 "[x]"
  FileClose $0
  WriteRegStr HKLM "Software\OddVendor\DemoSettings" "Path" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RidTraceDemo" "DisplayName" "Rid Trace Demo"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RidTraceDemo" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RidTraceDemo" "InstallLocation" "$INSTDIR"
  CreateShortcut "$SMPROGRAMS\Rid Trace Demo.lnk" "$INSTDIR\demo.txt"
  WriteUninstaller "$INSTDIR\uninstall.exe"
SectionEnd

; Leaves the data folder and the vendor key behind on purpose, as real uninstallers do.
Section "Uninstall"
  SetShellVarContext all
  Delete "$SMPROGRAMS\Rid Trace Demo.lnk"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RidTraceDemo"
  RMDir /r "$INSTDIR"
SectionEnd
