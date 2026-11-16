#!/bin/sh
# CI guard (PRD §10): `unsafe` code may exist only under src/platform/win32/,
# and every `unsafe` block/fn there must carry a `// SAFETY:` comment.
set -eu
cd "$(dirname "$0")/.."
status=0

# 1. No unsafe items/blocks outside the FFI module.
if grep -rnE 'unsafe[[:space:]]*(\{|fn[[:space:]]|impl[[:space:]]|trait[[:space:]]|extern)' src \
     --include='*.rs' | grep -v '^src/platform/win32/'; then
  echo "error: unsafe code outside src/platform/win32/" >&2
  status=1
fi
# 2. Nobody else may re-allow the lint.
if grep -rn 'allow(unsafe_code)' src --include='*.rs' | grep -v '^src/platform/win32/mod.rs' \
     | grep -vE ':[0-9]+:[[:space:]]*//'; then
  echo "error: allow(unsafe_code) outside src/platform/win32/mod.rs" >&2
  status=1
fi
# 3. Every `unsafe {` / `unsafe fn` in win32 has a SAFETY (or `# Safety`) note in the 12 lines above.
for f in $(find src/platform/win32 -name '*.rs'); do
  awk -v file="$f" '
    { lines[NR] = $0 }
    /unsafe[ \t]*\{|unsafe[ \t]+(extern|fn)/ && !/^[ \t]*\/\// {
      ok = 0
      for (i = NR - 12; i < NR; i++) if (i > 0 && lines[i] ~ /SAFETY|# Safety/) ok = 1
      if (!ok) { printf "error: %s:%d: unsafe without a nearby // SAFETY: comment\n", file, NR > "/dev/stderr"; bad = 1 }
    }
    END { exit bad }
  ' "$f" || status=1
done
[ "$status" -eq 0 ] && echo "unsafe guard: ok"
exit "$status"
