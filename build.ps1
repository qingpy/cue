# Release build. w64devkit supplies as/dlltool/gcc/ld, which the rustup
# windows-gnu toolchain omits; its libgcc_eh.a is an empty stub we created
# (w64devkit's unwinder lives inside libgcc.a).
$env:PATH = "$env:LOCALAPPDATA\w64devkit\bin;$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo build --release
# WebView2Loader.dll must sit beside the exe (import-table dependency);
# deploy it together with cue.exe
$dll = Get-ChildItem "$env:USERPROFILE\.cargo\registry\src" -Recurse -Filter WebView2Loader.dll |
    Where-Object FullName -match '\\x64\\' | Select-Object -First 1
if ($dll) { Copy-Item $dll.FullName target\release\ -Force }
