; Installer hooks (bundle.windows.nsis.installerHooks): Arcalo Mail registers as a mail client,
; so Windows offers it for e-mail links (mailto:) under Settings → Apps → Default apps. Windows
; lets only the user choose the default; the setup never takes mailto: over (the app's
; „Als Standard-Mail-App festlegen“ opens that settings page, src-tauri/src/defaultmail.rs).
;
; - The class ArcaloMail.mailto starts the program with the address („%1“).
; - Clients\Mail\Arcalo Mail with its capabilities (mailto → ArcaloMail.mailto) and the entry
;   in RegisteredApplications make it a choice in Default apps.
;
; SHCTX is HKCU here (installMode currentUser). Running these steps again (an update) writes the
; same values; uninstalling removes them, except when the uninstaller runs for an update.

!define MAIL_CLASS "Software\Classes\ArcaloMail.mailto"
!define MAIL_CLIENT "Software\Clients\Mail\Arcalo Mail"

!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr SHCTX "${MAIL_CLASS}" "" "URL:MailTo Protocol"
  WriteRegStr SHCTX "${MAIL_CLASS}" "URL Protocol" ""
  WriteRegStr SHCTX "${MAIL_CLASS}" "FriendlyTypeName" "Arcalo Mail"
  WriteRegStr SHCTX "${MAIL_CLASS}\DefaultIcon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
  WriteRegStr SHCTX "${MAIL_CLASS}\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
  WriteRegStr SHCTX "${MAIL_CLIENT}" "" "Arcalo Mail"
  WriteRegStr SHCTX "${MAIL_CLIENT}\DefaultIcon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
  WriteRegStr SHCTX "${MAIL_CLIENT}\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\""
  WriteRegStr SHCTX "${MAIL_CLIENT}\Capabilities" "ApplicationName" "Arcalo Mail"
  WriteRegStr SHCTX "${MAIL_CLIENT}\Capabilities" "ApplicationDescription" "Local-first e-mail from MouseWerk"
  WriteRegStr SHCTX "${MAIL_CLIENT}\Capabilities" "ApplicationIcon" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
  WriteRegStr SHCTX "${MAIL_CLIENT}\Capabilities\URLAssociations" "mailto" "ArcaloMail.mailto"
  WriteRegStr SHCTX "Software\RegisteredApplications" "Arcalo Mail" "${MAIL_CLIENT}\Capabilities"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    DeleteRegKey SHCTX "${MAIL_CLASS}"
    DeleteRegKey SHCTX "${MAIL_CLIENT}"
    DeleteRegValue SHCTX "Software\RegisteredApplications" "Arcalo Mail"
  ${EndIf}
!macroend
