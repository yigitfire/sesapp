; Uygulamanın eski adı "Ses" idi. Windows kurulum klasörünü ve Başlat menüsü kaydını ada göre
; tuttuğu için Monke, eskisinin üstüne değil yanına (%LOCALAPPDATA%\Monke) kurulur. Eski "Ses"
; kurulu kalırsa iki uygulama olur; bu yüzden kurulum bitince eskisi sessizce kaldırılır.
;
; Veriler (mesajlar, profil, oda) SİLİNMEZ: sessiz kaldırmada "uygulama verilerini sil"
; seçili değildir; veriler zaten ada değil uygulama kimliğine (io.github.yigitfire.sesapp) bağlı.
;
; Otomatik güncellemede kurulum programı kısayol oluşturmaz (eskiler duruyor sanar). Eski
; uygulama kaldırılınca onun kısayolları da gittiği için yenileri burada oluşturulur.
!macro NSIS_HOOK_POSTINSTALL
  ReadRegStr $R8 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Ses" "InstallLocation"
  ${If} $R8 != ""
    ; Kayıtta tırnak içinde yazılı: "C:\...\Ses"
    StrCpy $R9 $R8 1
    ${If} $R9 == '"'
      StrCpy $R8 $R8 -1 1
    ${EndIf}
    ${If} $R8 != $INSTDIR
    ${AndIf} ${FileExists} "$R8\uninstall.exe"
      ; Eskinin masaüstü kısayolu var mıydı? Varsa yenisi de masaüstüne konur.
      StrCpy $R9 0
      ${If} ${FileExists} "$DESKTOP\Ses.lnk"
        StrCpy $R9 1
      ${EndIf}
      ; _?= : kaldırıcı kendini kopyalamadan, burada bitene kadar bekleyerek çalışır.
      ExecWait '"$R8\uninstall.exe" /S _?=$R8'
      Delete "$R8\uninstall.exe"
      RMDir "$R8"
      ${IfNot} ${FileExists} "$SMPROGRAMS\${PRODUCTNAME}.lnk"
        CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
        !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${PRODUCTNAME}.lnk"
      ${EndIf}
      ${If} $R9 = 1
      ${AndIfNot} ${FileExists} "$DESKTOP\${PRODUCTNAME}.lnk"
        CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
        !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend
