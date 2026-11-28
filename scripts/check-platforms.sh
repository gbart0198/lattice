#!/usr/bin/env bash
# Checks that lattice still builds for both platforms from one tree:
#   1. the host build (macOS or Linux): every target compiles without warnings and the tests pass;
#   2. Windows: `cargo check` for x86_64-pc-windows-msvc, without warnings;
#   3. Win32 stays inside the platform folders: no `windows::` / `std::os::windows` elsewhere.
# On a Mac, step 2 needs no MSVC toolchain: `cargo check` never links, so the C and assembly
# that a few dependencies build (psm, stacker) are faked by scripts/msvc-stub.sh, and blake3 is
# told to skip its assembly. On Windows, step 1 is the Windows build and step 2 is skipped.
set -euo pipefail
cd "$(dirname "$0")/.."

export RUSTFLAGS="${RUSTFLAGS:-} -D warnings"

echo "== 1. host: cargo test --all-targets"
cargo test --all-targets --quiet

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) echo "== 2. skipped: this is Windows, step 1 covered it" ;;
  *)
    echo "== 2. windows: cargo check --target x86_64-pc-windows-msvc --all-targets"
    stub=$(mktemp -d)
    trap 'rm -rf "$stub"' EXIT
    for t in cl.exe lib.exe ml64.exe rc.exe llvm-rc llvm-lib; do ln -s "$PWD/scripts/msvc-stub.sh" "$stub/$t"; done
    PATH="$stub:$PATH" CARGO_FEATURE_PURE=1 \
      CC_x86_64_pc_windows_msvc="$stub/cl.exe" AR_x86_64_pc_windows_msvc="$stub/lib.exe" \
      cargo check --target x86_64-pc-windows-msvc --all-targets --quiet
    ;;
esac

echo "== 3. Win32 only under platform/windows/"
# lattice-console is the Windows terminal shim (lattice.com) and has no other platform.
leaks=$(grep -rnE '\bwindows::|std::os::windows' src --include='*.rs' \
  | grep -vE '^src/(.+/)?platform/windows/' \
  | grep -v '^src/bin/lattice-console.rs:' || true)
if [ -n "$leaks" ]; then
  echo "Win32 outside a platform/windows/ folder:"; echo "$leaks"; exit 1
fi
echo "ok"
