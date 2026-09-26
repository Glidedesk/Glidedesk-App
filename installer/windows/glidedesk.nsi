; Glidedesk installer / upgrader / uninstaller.
;
; One setup.exe handles every case:
;   nothing installed  -> Install     (wizard)
;   older version      -> Upgrade     (short confirmation, same folder, settings untouched)
;   same version       -> Repair
;   newer version      -> blocked     (/ALLOWDOWNGRADE overrides)
;   other architecture -> Replace     (old build removed, settings kept)
; Upgrades stop the app cleanly, back up the program folder, and roll back
; automatically if copying fails.
;
; Silent use:  setup.exe /S [/D=C:\path] [/NORESTART] [/ALLOWDOWNGRADE]
;              uninstall.exe /S [/KEEPCONFIG=0|1]
; Exit code: 0 = success, 1 = cancelled/failed (nothing changed), 2 = blocked.
;
; Build-time defines (from `cargo xtask package windows`):
;   VERSION ARCH APP_EXE ICON WEBVIEW2 WEBVIEW2_OFFLINE OUTFILE
; Exit code 3 = installed, but with a warning (firewall rule or WebView2 failed).

Unicode true
!ifdef SIGN
  ; Sign the uninstaller and the finished installer with the self-signed certificate.
  !uninstfinalize 'gd-sign-windows "%1"' = 0
  !finalize 'gd-sign-windows "%1"' = 0
!endif
ManifestDPIAware true
ManifestSupportedOS all
RequestExecutionLevel admin
SetCompressor /SOLID lzma
SetCompressorDictSize 32

!include "MUI2.nsh"
!include "nsDialogs.nsh"
!include "LogicLib.nsh"
!include "WordFunc.nsh"
!include "FileFunc.nsh"
!include "x64.nsh"
!include "WinVer.nsh"

!define PRODUCT "Glidedesk"
!define PUBLISHER "Glidedesk"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Glidedesk"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define WV2_GUID "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
!if ${WEBVIEW2_OFFLINE} == 1
  !define WV2_HINT ""
!else
  !define WV2_HINT "$\r$\nUse the offline installer on computers without internet."
!endif

Name "${PRODUCT}"
OutFile "${OUTFILE}"
InstallDir "$PROGRAMFILES64\${PRODUCT}"
BrandingText "${PRODUCT} ${VERSION}"
ShowInstDetails nevershow
ShowUninstDetails nevershow

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${PRODUCT}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "${PRODUCT} setup"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "LegalCopyright" "MIT OR Apache-2.0"

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "Start ${PRODUCT}"
!define MUI_FINISHPAGE_RUN_FUNCTION LaunchAsUser

Var Mode            ; install | upgrade | repair | replace
Var OldVersion
Var OldDir
Var OldArch
Var WasRunning      ; 1 if the app was running before an upgrade
Var NoRestart
Var DesktopShortcut
Var KeepConfig      ; uninstaller: 1 = keep settings
Var Dialog
Var Check
Var RadioKeep
Var Warn            ; 0 = clean install, 3 = installed with a warning

; ---------------------------------------------------------------------------
; pages
; ---------------------------------------------------------------------------

!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfNotInstall
!insertmacro MUI_PAGE_WELCOME
Page custom UpgradePage
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfNotInstall
!insertmacro MUI_PAGE_DIRECTORY
Page custom OptionsPage OptionsLeave
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

UninstPage custom un.KeepPage un.KeepLeave
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

; ---------------------------------------------------------------------------
; helpers
; ---------------------------------------------------------------------------

Function SkipIfNotInstall
  ${If} $Mode != "install"
    Abort
  ${EndIf}
FunctionEnd

; Ask a running agent to quit (releases keys, tells other computers), then
; make sure nothing of ours is left running so files can be replaced.
!macro StopApp DIR UN
  ; Graceful: the agent releases keys, says goodbye to other computers and
  ; finishes writing files. `--shutdown` waits up to 10 s for it to exit.
  ${If} ${FileExists} "${DIR}\glidedesk.exe"
    nsExec::Exec '"${DIR}\glidedesk.exe" --shutdown'
    Pop $0
  ${ElseIf} ${FileExists} "${DIR}\glidedesk-agent.exe"
    nsExec::Exec '"${DIR}\glidedesk-agent.exe" --shutdown'
    Pop $0
  ${EndIf}
  ; Then ask the tray app to close, and wait up to 10 s before forcing.
  nsExec::Exec 'taskkill /IM glidedesk.exe /T'
  Pop $0
  StrCpy $R9 0
  ${Do}
    Call ${UN}IsAppRunning
    Pop $R8
    ${If} $R8 == "0"
      ${ExitDo}
    ${EndIf}
    Sleep 500
    IntOp $R9 $R9 + 1
  ${LoopUntil} $R9 >= 20
  ${If} $R8 == "1"
    DetailPrint "${PRODUCT} did not close in time; forcing it to stop."
    nsExec::Exec 'taskkill /F /IM glidedesk.exe /T'
    Pop $0
  ${EndIf}
  nsExec::Exec 'taskkill /F /IM glidedesk-agent.exe /T'
  Pop $0
  Sleep 300
!macroend

!macro IsAppRunningBody
  ; Returns "1" or "0" on the stack.
  nsExec::ExecToStack 'tasklist /FI "IMAGENAME eq glidedesk.exe" /NH'
  Pop $0
  Pop $1
  ${WordFind} "$1" "glidedesk.exe" "E+1{" $2
  ${If} ${Errors}
    Push "0"
  ${Else}
    Push "1"
  ${EndIf}
!macroend
Function IsAppRunning
  !insertmacro IsAppRunningBody
FunctionEnd
Function un.IsAppRunning
  !insertmacro IsAppRunningBody
FunctionEnd

; Start the app as the signed-in user, not elevated (the installer runs as admin).
Function LaunchAsUser
  Exec '"$WINDIR\explorer.exe" "$INSTDIR\glidedesk.exe"'
FunctionEnd

Function EnsureWebView2
  ReadRegStr $0 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\${WV2_GUID}" "pv"
  ${If} $0 == ""
    ReadRegStr $0 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WV2_GUID}" "pv"
  ${EndIf}
  ${If} $0 == ""
    ReadRegStr $0 HKCU "Software\Microsoft\EdgeUpdate\Clients\${WV2_GUID}" "pv"
  ${EndIf}
  ${If} $0 == ""
  ${OrIf} $0 == "0.0.0.0"
    DetailPrint "Installing Microsoft Edge WebView2 runtime…"
    SetOutPath "$PLUGINSDIR"
    File /oname=webview2-setup.exe "${WEBVIEW2}"
    ExecWait '"$PLUGINSDIR\webview2-setup.exe" /silent /install' $1
    ${If} $1 != 0
      DetailPrint "WebView2 runtime install failed (code $1)."
      StrCpy $Warn 3
      MessageBox MB_ICONEXCLAMATION|MB_OK "The WebView2 runtime could not be installed (code $1).$\r$\nGlidedesk's settings window needs it.${WV2_HINT}" /SD IDOK
    ${EndIf}
  ${EndIf}
FunctionEnd

; ---------------------------------------------------------------------------
; detection
; ---------------------------------------------------------------------------

Function .onInit
  SetShellVarContext all
  SetRegView 64
  InitPluginsDir

  ${IfNot} ${AtLeastWin10}
    MessageBox MB_ICONSTOP "${PRODUCT} needs Windows 10, Windows 11 or Windows Server 2016 or later." /SD IDOK
    SetErrorLevel 2
    Quit
  ${EndIf}
  !if "${ARCH}" == "arm64"
    ${IfNot} ${IsNativeARM64}
      MessageBox MB_ICONSTOP "This installer is for ARM64 Windows. Please use the x64 installer." /SD IDOK
      SetErrorLevel 2
      Quit
    ${EndIf}
  !else
    ${IfNot} ${RunningX64}
      MessageBox MB_ICONSTOP "${PRODUCT} needs 64-bit Windows." /SD IDOK
      SetErrorLevel 2
      Quit
    ${EndIf}
  !endif

  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/NORESTART" $0
  ${IfNot} ${Errors}
    StrCpy $NoRestart 1
  ${EndIf}
  StrCpy $DesktopShortcut 0

  ReadRegStr $OldVersion HKLM "${UNINST_KEY}" "DisplayVersion"
  ReadRegStr $OldDir HKLM "${UNINST_KEY}" "InstallLocation"
  ReadRegStr $OldArch HKLM "${UNINST_KEY}" "Architecture"

  ${If} $OldVersion == ""
  ${OrIfNot} ${FileExists} "$OldDir\glidedesk.exe"
    StrCpy $Mode "install"
    Return
  ${EndIf}

  StrCpy $INSTDIR $OldDir
  ${If} $OldArch != ""
  ${AndIf} $OldArch != "${ARCH}"
    StrCpy $Mode "replace"
    Return
  ${EndIf}
  ${VersionCompare} "${VERSION}" "$OldVersion" $0
  ${If} $0 == 0
    StrCpy $Mode "repair"
  ${ElseIf} $0 == 1
    StrCpy $Mode "upgrade"
  ${Else}
    ClearErrors
    ${GetOptions} $R0 "/ALLOWDOWNGRADE" $1
    ${If} ${Errors}
      MessageBox MB_ICONEXCLAMATION "A newer ${PRODUCT} ($OldVersion) is already installed.$\r$\nThis installer has version ${VERSION}." /SD IDOK
      SetErrorLevel 2
      Quit
    ${EndIf}
    StrCpy $Mode "upgrade"
  ${EndIf}
FunctionEnd

Function UpgradePage
  ${If} $Mode == "install"
    Abort
  ${EndIf}
  !insertmacro MUI_HEADER_TEXT "Update ${PRODUCT}" "Your settings, layout and computers are kept."
  nsDialogs::Create 1018
  Pop $Dialog
  ${If} $Mode == "upgrade"
    ${NSD_CreateLabel} 0 0 100% 36u "${PRODUCT} $OldVersion is installed in:$\r$\n$INSTDIR$\r$\n$\r$\nClick Next to update it to ${VERSION}."
  ${ElseIf} $Mode == "repair"
    ${NSD_CreateLabel} 0 0 100% 36u "${PRODUCT} ${VERSION} is already installed in:$\r$\n$INSTDIR$\r$\n$\r$\nClick Next to repair it (files, shortcuts, firewall rule)."
  ${Else}
    ${NSD_CreateLabel} 0 0 100% 36u "A $OldArch build of ${PRODUCT} is installed.$\r$\nClick Next to replace it with the ${ARCH} build ${VERSION}."
  ${EndIf}
  Pop $0
  ${NSD_CreateLabel} 0 50u 100% 24u "${PRODUCT} will close for a moment and start again when the update is done."
  Pop $0
  nsDialogs::Show
FunctionEnd

Function OptionsPage
  ${If} $Mode != "install"
    Abort
  ${EndIf}
  !insertmacro MUI_HEADER_TEXT "Options" "Choose extra shortcuts."
  nsDialogs::Create 1018
  Pop $Dialog
  ${NSD_CreateCheckbox} 0 0 100% 12u "Create a desktop shortcut"
  Pop $Check
  ${NSD_CreateLabel} 0 24u 100% 36u "${PRODUCT} starts when you sign in (you can turn this off in its settings).$\r$\nA firewall rule for private networks is added so other computers can connect."
  Pop $0
  nsDialogs::Show
FunctionEnd

Function OptionsLeave
  ${NSD_GetState} $Check $DesktopShortcut
FunctionEnd

; ---------------------------------------------------------------------------
; install / upgrade
; ---------------------------------------------------------------------------

Section "Install"
  SetShellVarContext all
  SetRegView 64
  StrCpy $WasRunning 0
  StrCpy $Warn 0
  ${If} $Mode != "install"
    Call IsAppRunning
    Pop $WasRunning
    DetailPrint "Stopping ${PRODUCT}…"
    !insertmacro StopApp "$INSTDIR" ""
  ${EndIf}

  ${If} $Mode == "replace"
    ; Remove the other-architecture build but keep the settings.
    ${If} ${FileExists} "$OldDir\uninstall.exe"
      CopyFiles /SILENT "$OldDir\uninstall.exe" "$PLUGINSDIR\old-uninstall.exe"
      ExecWait '"$PLUGINSDIR\old-uninstall.exe" /S /KEEPCONFIG=1 _?=$OldDir' $0
    ${EndIf}
  ${EndIf}

  ; Back up the current program folder for automatic rollback.
  RMDir /r "$INSTDIR.old"
  ${If} ${FileExists} "$INSTDIR\glidedesk.exe"
    ClearErrors
    Rename "$INSTDIR" "$INSTDIR.old"
    ${If} ${Errors}
      MessageBox MB_ICONSTOP "${PRODUCT} files are still in use. Close ${PRODUCT} and run setup again." /SD IDOK
      SetErrorLevel 1
      Abort
    ${EndIf}
  ${EndIf}

  ClearErrors
  SetOutPath "$INSTDIR"
  SetOverwrite on
  File /oname=glidedesk.exe "${APP_EXE}"
  File /oname=glidedesk.ico "${ICON}"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  ${If} ${Errors}
    DetailPrint "Copy failed — restoring the previous version."
    RMDir /r "$INSTDIR"
    ${If} ${FileExists} "$INSTDIR.old\glidedesk.exe"
      Rename "$INSTDIR.old" "$INSTDIR"
      ${If} $WasRunning == 1
        Call LaunchAsUser
      ${EndIf}
    ${EndIf}
    MessageBox MB_ICONSTOP "Installing ${PRODUCT} failed; nothing was changed." /SD IDOK
    SetErrorLevel 1
    Abort
  ${EndIf}

  Call EnsureWebView2

  ; Add/Remove Programs entry.
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayName" "${PRODUCT}"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINST_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\glidedesk.ico"
  WriteRegStr HKLM "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINST_KEY}" "Architecture" "${ARCH}"
  WriteRegStr HKLM "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKLM "${UNINST_KEY}" "EstimatedSize" "$0"
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoRepair" 1

  ; Start at login (fresh installs; the app's own setting manages it afterwards).
  ${If} $Mode == "install"
    WriteRegStr HKCU "${RUN_KEY}" "${PRODUCT}" '"$INSTDIR\glidedesk.exe" --background'
  ${EndIf}

  ; Shortcuts.
  CreateShortcut "$SMPROGRAMS\${PRODUCT}.lnk" "$INSTDIR\glidedesk.exe" "" "$INSTDIR\glidedesk.ico"
  ${If} $DesktopShortcut == ${BST_CHECKED}
    CreateShortcut "$DESKTOP\${PRODUCT}.lnk" "$INSTDIR\glidedesk.exe" "" "$INSTDIR\glidedesk.ico"
  ${EndIf}

  ; Firewall: inbound UDP for the agent, private + domain networks only.
  nsExec::Exec 'netsh advfirewall firewall delete rule name="${PRODUCT}"'
  Pop $0
  nsExec::Exec 'netsh advfirewall firewall add rule name="${PRODUCT}" dir=in action=allow program="$INSTDIR\glidedesk.exe" protocol=UDP profile=private,domain enable=yes'
  Pop $0
  ${If} $0 != 0
    DetailPrint "Adding the firewall rule failed (code $0)."
    StrCpy $Warn 3
    MessageBox MB_ICONEXCLAMATION|MB_OK "Windows Firewall rule for ${PRODUCT} could not be added (code $0).$\r$\nOther computers may not be able to connect until you allow ${PRODUCT} on private networks." /SD IDOK
  ${EndIf}
  ; Remove the separate agent of versions before 0.2.
  Delete "$INSTDIR\glidedesk-agent.exe"

  ; Success: drop the backup.
  RMDir /r "$INSTDIR.old"

  ; An update restarts the app if it was running (unless /NORESTART).
  ${If} $Mode != "install"
  ${AndIf} $WasRunning == 1
  ${AndIf} $NoRestart != 1
    Call LaunchAsUser
  ${EndIf}
  ; Silent fresh installs start the app too (servers deployed by script).
  ${If} ${Silent}
  ${AndIf} $Mode == "install"
  ${AndIf} $NoRestart != 1
    Call LaunchAsUser
  ${EndIf}
  SetErrorLevel $Warn
SectionEnd

Function .onInstFailed
  SetErrorLevel 1
FunctionEnd

; ---------------------------------------------------------------------------
; uninstall
; ---------------------------------------------------------------------------

Function un.onInit
  SetShellVarContext all
  SetRegView 64
  StrCpy $KeepConfig 1
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/KEEPCONFIG=" $0
  ${IfNot} ${Errors}
    ${If} $0 == "0"
      StrCpy $KeepConfig 0
    ${EndIf}
  ${EndIf}
FunctionEnd

Function un.KeepPage
  !insertmacro MUI_HEADER_TEXT "Uninstall ${PRODUCT}" "Keep your settings for a future install?"
  nsDialogs::Create 1018
  Pop $Dialog
  ${NSD_CreateRadioButton} 0 0 100% 12u "Keep my settings and layout (recommended)"
  Pop $RadioKeep
  ${NSD_CreateRadioButton} 0 16u 100% 12u "Remove my settings, layout and logs"
  Pop $0
  ${If} $KeepConfig == 1
    ${NSD_Check} $RadioKeep
  ${Else}
    ${NSD_Check} $0
  ${EndIf}
  ${NSD_CreateLabel} 0 40u 100% 24u "Settings are stored in your user profile ($APPDATA\${PRODUCT})."
  Pop $0
  nsDialogs::Show
FunctionEnd

Function un.KeepLeave
  ${NSD_GetState} $RadioKeep $KeepConfig
  ${If} $KeepConfig == ${BST_CHECKED}
    StrCpy $KeepConfig 1
  ${Else}
    StrCpy $KeepConfig 0
  ${EndIf}
FunctionEnd

Section "Uninstall"
  SetShellVarContext all
  SetRegView 64
  !insertmacro StopApp "$INSTDIR" "un."

  Delete "$INSTDIR\glidedesk.exe"
  Delete "$INSTDIR\glidedesk-agent.exe"
  Delete "$INSTDIR\glidedesk.ico"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  RMDir /r "$INSTDIR.old"

  Delete "$SMPROGRAMS\${PRODUCT}.lnk"
  SetShellVarContext current
  Delete "$DESKTOP\${PRODUCT}.lnk"
  SetShellVarContext all
  Delete "$DESKTOP\${PRODUCT}.lnk"

  nsExec::Exec 'netsh advfirewall firewall delete rule name="${PRODUCT}"'
  Pop $0
  DeleteRegKey HKLM "${UNINST_KEY}"
  ; Start-at-login entry written by the app for this user.
  DeleteRegValue HKCU "${RUN_KEY}" "${PRODUCT}"
  DeleteRegValue HKCU "${RUN_KEY}" "glidedesk"

  ; Web view cache is never kept.
  SetShellVarContext current
  RMDir /r "$LOCALAPPDATA\app.glidedesk.desktop"
  ${If} $KeepConfig == 0
    RMDir /r "$APPDATA\${PRODUCT}"
    RMDir /r "$LOCALAPPDATA\${PRODUCT}"
  ${Else}
    ; Logs are not settings.
    RMDir /r "$LOCALAPPDATA\${PRODUCT}\logs"
  ${EndIf}
  SetShellVarContext all
  SetErrorLevel 0
SectionEnd
