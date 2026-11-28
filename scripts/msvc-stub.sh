#!/usr/bin/env bash
# Stand-in for an MSVC tool (cl, lib, ml64, rc) in cross *checks* from macOS or Linux only
# (scripts/check-platforms.sh): writes the empty output file it was asked for and succeeds.
# Never use it for a real build: the objects it makes are empty.
for a in "$@"; do
  case "$a" in
    /Fo*|-Fo*|/fo*|-fo*) f="${a:3}" ;;
    /OUT:*|-OUT:*|/out:*|-out:*) f="${a#*:}" ;;
    *) continue ;;
  esac
  [ -n "$f" ] && mkdir -p "$(dirname "$f")" && : > "$f"
done
exit 0
