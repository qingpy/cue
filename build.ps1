# Release build. w64devkit supplies as/dlltool/gcc/ld, which the rustup
# windows-gnu toolchain omits; its libgcc_eh.a is an empty stub we created
# (w64devkit's unwinder lives inside libgcc.a).
$env:PATH = "$env:LOCALAPPDATA\w64devkit\bin;$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo build --release
