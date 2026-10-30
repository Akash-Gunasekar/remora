; sherpa-onnx (Parakeet / Moonshine) ships as DLLs that must sit next to remora.exe.
; Paths are relative to the generated installer.nsi in target\release\nsis\x64.
!macro NSIS_HOOK_POSTINSTALL
  SetOutPath "$INSTDIR"
  File "..\..\sherpa-onnx-c-api.dll"
  File "..\..\sherpa-onnx-cxx-api.dll"
  File "..\..\onnxruntime.dll"
  File "..\..\onnxruntime_providers_shared.dll"
  File "..\..\cargs.dll"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  Delete "$INSTDIR\sherpa-onnx-c-api.dll"
  Delete "$INSTDIR\sherpa-onnx-cxx-api.dll"
  Delete "$INSTDIR\onnxruntime.dll"
  Delete "$INSTDIR\onnxruntime_providers_shared.dll"
  Delete "$INSTDIR\cargs.dll"
!macroend
