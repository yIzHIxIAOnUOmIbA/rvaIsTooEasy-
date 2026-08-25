$nodeDir='C:\Users\hello\.workbuddy\binaries\node\versions\22.22.2'
$env:PATH=$nodeDir+';'+$env:PATH
$env:NODE_OPTIONS=''
cd 'C:\Users\hello\Documents\GitHub\rvaIsTooEasy\crates\rva_gui'
if (Test-Path build){Remove-Item build -Recurse -Force -ErrorAction SilentlyContinue}
npm run tauri build *> build_rva.log
Write-Output ('EXIT '+$LASTEXITCODE) | Out-File -Append build_rva.log
