#!/usr/bin/env bash
#
# Prepare fixtures (ASR models + speech audio) for the transcription quality
# harness at src-tauri/examples/tc_quality.rs.
#
#     scripts/prepare-asr-fixtures.sh              # models + audio (incl. sensevoice)
#     ASR_SKIP_SENSEVOICE=1 scripts/prepare-asr-fixtures.sh   # skip the 252MB model
#     scripts/prepare-asr-fixtures.sh --no-sensevoice        # same, via flag
#     ASR_FIXTURES_DIR=/some/dir scripts/prepare-asr-fixtures.sh
#
# Idempotent: any file that already exists (and, for downloads, matches the
# expected byte size) is left untouched. Then run:
#
#     cd src-tauri && ASR_FIXTURES_DIR="${ASR_FIXTURES_DIR:-/tmp}" \
#         cargo run --example tc_quality
#
# The Chinese sample is synthesized locally with macOS `say` + `afconvert`, so
# audio generation is macOS-only. The English sample and the GGUF models are
# plain HTTPS downloads and work anywhere `curl` does.

set -euo pipefail

DIR="${ASR_FIXTURES_DIR:-/tmp}"

# The exact Chinese sentence tc_quality.rs expects for zh.wav.
ZH_TEXT="今天天气很好，我们一起去公园散步吧"

SKIP_SENSEVOICE="${ASR_SKIP_SENSEVOICE:-0}"
for arg in "$@"; do
  case "$arg" in
    --no-sensevoice) SKIP_SENSEVOICE=1 ;;
    -h | --help)
      grep '^#' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "unknown argument: $arg" >&2
      exit 2
      ;;
  esac
done

mkdir -p "$DIR"

# --- helpers ---------------------------------------------------------------

# Byte size of a file, or empty string if it does not exist (macOS/BSD + Linux).
# `-L` follows symlinks so a symlinked fixture reports its target's real size.
file_size() {
  if [ -e "$1" ]; then
    stat -Lf%z "$1" 2>/dev/null || stat -Lc%s "$1" 2>/dev/null || echo ""
  fi
}

# Human-readable size.
human() {
  awk -v b="$1" 'BEGIN{
    if (b=="") { print "?"; exit }
    split("B KB MB GB", u, " "); i=1;
    while (b>=1024 && i<4) { b/=1024; i++ }
    printf (i==1 ? "%d %s" : "%.1f %s"), b, u[i]
  }'
}

report() {
  local path="$1"
  local size
  size="$(file_size "$path")"
  if [ -n "$size" ]; then
    printf '  %-28s %10s  %s\n' "$(basename "$path")" "$(human "$size")" "$path"
  else
    printf '  %-28s %10s  %s\n' "$(basename "$path")" "MISSING" "$path"
  fi
}

# download URL DEST EXPECTED_BYTES
# Skips when DEST already exists with the expected size; otherwise (re)downloads
# with resume support.
download() {
  local url="$1" dest="$2" expect="${3:-}"
  local have
  have="$(file_size "$dest")"
  if [ -n "$have" ] && { [ -z "$expect" ] || [ "$have" = "$expect" ]; }; then
    echo "  skip (exists): $(basename "$dest")"
    return 0
  fi
  if [ -n "$have" ] && [ -n "$expect" ] && [ "$have" != "$expect" ]; then
    echo "  re-download (size $have != expected $expect): $(basename "$dest")"
  else
    echo "  downloading: $(basename "$dest")"
  fi
  curl -fL --retry 3 --retry-delay 2 -C - -o "$dest" "$url"
}

# --- audio: zh.wav (synthesized) -------------------------------------------

echo "== audio =="
ZH_WAV="$DIR/zh.wav"
if [ -f "$ZH_WAV" ]; then
  echo "  skip (exists): zh.wav"
elif command -v say >/dev/null 2>&1 && command -v afconvert >/dev/null 2>&1; then
  echo "  synthesizing zh.wav via say -v Tingting"
  TMP_AIFF="$(mktemp -t zh_fixture).aiff"
  say -v Tingting -o "$TMP_AIFF" "$ZH_TEXT"
  # 16 kHz, mono, signed 16-bit little-endian PCM WAV — what the harness expects.
  afconvert -f WAVE -d LEI16@16000 -c 1 "$TMP_AIFF" "$ZH_WAV"
  rm -f "$TMP_AIFF"
else
  echo "  WARN: say/afconvert unavailable (macOS only) — cannot synthesize zh.wav" >&2
fi

# --- audio: jfk.wav (downloaded) -------------------------------------------

download \
  "https://raw.githubusercontent.com/handy-computer/transcribe.cpp/main/samples/jfk.wav" \
  "$DIR/jfk.wav" \
  "352078"

# --- models ----------------------------------------------------------------

echo "== models =="

# moonshine-tiny-zh (Q8_0, ~34MB) — Chinese moonshine.
download \
  "https://huggingface.co/handy-computer/moonshine-tiny-zh-gguf/resolve/main/moonshine-tiny-zh-Q8_0.gguf" \
  "$DIR/moonshine-tiny-zh.gguf" \
  "35466944"

# whisper-base (Q8_0, ~81MB) — multilingual whisper.
download \
  "https://huggingface.co/handy-computer/whisper-base-gguf/resolve/main/whisper-base-Q8_0.gguf" \
  "$DIR/whisper-base.gguf" \
  "84962880"

# SenseVoiceSmall (Q8_0, ~241MB) — optional, large.
if [ "$SKIP_SENSEVOICE" = "1" ]; then
  echo "  skip (ASR_SKIP_SENSEVOICE): sensevoice.gguf"
else
  download \
    "https://huggingface.co/handy-computer/SenseVoiceSmall-gguf/resolve/main/SenseVoiceSmall-Q8_0.gguf" \
    "$DIR/sensevoice.gguf" \
    "252684608"
fi

# --- summary ---------------------------------------------------------------

echo ""
echo "== fixtures in $DIR =="
report "$DIR/zh.wav"
report "$DIR/jfk.wav"
report "$DIR/moonshine-tiny-zh.gguf"
report "$DIR/whisper-base.gguf"
report "$DIR/sensevoice.gguf"
echo ""
echo "Run: cd src-tauri && ASR_FIXTURES_DIR=$DIR cargo run --example tc_quality"
