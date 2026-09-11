#!/usr/bin/env bash
#
# Local inference: llama.cpp, whisper.cpp, piper, openWakeWord.
#
# llama.cpp is built here rather than installed, because the build flags are the
# whole point. A generic binary runs on the CPU; one built with CUDA for compute
# 6.1 runs on the GTX 1080 that the reference machine has sitting idle. That
# difference is the difference between XOS being usable on old hardware and not.
#
# Every build here is allowed to fail. A machine with no CUDA, or no compiler
# that day, still gets a working desktop and XOS still works through an API.
# What it must not do is stop the install.

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"

PREFIX="$XOS_ROOT/opt/xos"
SOURCES="$PREFIX/src"

xstep "Build tools for inference"
pin_install_all cmake openblas || xsoft_fail "some build tools are missing"

# Compute 6.1 is Pascal. Building for it produces something every newer NVIDIA
# card also runs, so this is a floor rather than a ceiling.
CUDA_ARCH=61
if [ -f "$XOS_ROOT/etc/xos/cuda.env" ]; then
  # shellcheck disable=SC1090
  . "$XOS_ROOT/etc/xos/cuda.env"
fi

have_cuda() {
  [ -x "/opt/cuda/bin/nvcc" ] || command -v nvcc >/dev/null 2>&1
}

build_from_git() { # name, url, cmake flags...
  local name="$1"; local url="$2"; shift 2
  local directory="$SOURCES/$name"

  if [ "$XOS_DRY_RUN" = "1" ]; then
    xlog "   would build $name from $url with $*"
    return 0
  fi

  mkdir -p "$SOURCES" 2>/dev/null
  if [ -d "$directory/.git" ]; then
    git -C "$directory" pull --ff-only >> "$XOS_LOG" 2>&1 \
      || xwarn "could not update $name; building what is there"
  else
    git clone --depth 1 "$url" "$directory" >> "$XOS_LOG" 2>&1 \
      || { xsoft_fail "could not fetch $name"; return 1; }
  fi

  cmake -S "$directory" -B "$directory/build" "$@" >> "$XOS_LOG" 2>&1 \
    && cmake --build "$directory/build" --config Release -j "$(nproc 2>/dev/null || echo 2)" \
         >> "$XOS_LOG" 2>&1 \
    || { xsoft_fail "$name did not build"; return 1; }
  return 0
}

xstep "llama.cpp"

if have_cuda; then
  xlog "   building with CUDA for compute $CUDA_ARCH"
  build_from_git llama.cpp https://github.com/ggerganov/llama.cpp \
    -DGGML_CUDA=ON "-DCMAKE_CUDA_ARCHITECTURES=$CUDA_ARCH" -DCMAKE_BUILD_TYPE=Release \
    && xlog "   llama-server will use the GPU"
else
  # Not a failure. Plenty of target machines have no usable GPU, and llama.cpp
  # on a CPU with OpenBLAS is slow but real.
  xlog "   no CUDA here, so building for the CPU with OpenBLAS"
  build_from_git llama.cpp https://github.com/ggerganov/llama.cpp \
    -DGGML_BLAS=ON -DGGML_BLAS_VENDOR=OpenBLAS -DCMAKE_BUILD_TYPE=Release
fi

xstep "whisper.cpp"
if have_cuda; then
  build_from_git whisper.cpp https://github.com/ggerganov/whisper.cpp \
    -DGGML_CUDA=ON "-DCMAKE_CUDA_ARCHITECTURES=$CUDA_ARCH" -DCMAKE_BUILD_TYPE=Release
else
  build_from_git whisper.cpp https://github.com/ggerganov/whisper.cpp \
    -DCMAKE_BUILD_TYPE=Release
fi

xstep "piper"
# Speech out. Small enough to run on anything, which is the reason it is here
# rather than something better and heavier.
if [ "$XOS_DRY_RUN" = "1" ]; then
  xlog "   would install piper"
else
  if command -v uv >/dev/null 2>&1; then
    uv tool install piper-tts >> "$XOS_LOG" 2>&1 \
      || xsoft_fail "piper did not install; speech output will be unavailable"
  else
    xsoft_fail "uv is missing, so piper was skipped"
  fi
fi

xstep "openWakeWord"
if [ "$XOS_DRY_RUN" = "1" ]; then
  xlog "   would install openWakeWord"
else
  pin_install python-onnxruntime || xsoft_fail "onnxruntime is missing"
  if command -v uv >/dev/null 2>&1; then
    uv tool install openwakeword >> "$XOS_LOG" 2>&1 \
      || xsoft_fail "openWakeWord did not install; wake words will be unavailable"
  fi
fi

# llama-server is one of only two things XOS starts at boot. The other is xosd.
# Everything else waits to be asked.
xstep "The llama-server unit"
install_user_unit "xos-llama.service" <<'UNIT'
# Written by XOS.
#
# One of only two units enabled at boot, because the local model is what makes
# XOS work without the internet, and a model that has to load before it can
# answer is a model nobody waits for.
[Unit]
Description=XOS local model server
After=network.target

[Service]
Type=simple
EnvironmentFile=-%h/.config/xos/llama.env
ExecStart=/opt/xos/src/llama.cpp/build/bin/llama-server \
  --model ${XOS_MODEL_PATH} \
  --host 127.0.0.1 --port 8080 \
  --ctx-size ${XOS_CTX_SIZE} \
  --n-gpu-layers ${XOS_GPU_LAYERS} \
  --slots --cache-reuse 256
Restart=on-failure
RestartSec=5
# An old machine with one GPU should not be brought to its knees by the model
# server while someone is trying to use the desktop.
Nice=5

[Install]
WantedBy=default.target
UNIT

layer_into "etc/xos/llama.env.example" <<'ENV'
# Written by XOS. Copy to ~/.config/xos/llama.env and edit.
#
# Slot and prefix caching are what make an old GPU feel responsive, so the unit
# always passes --slots and --cache-reuse. These are the parts that depend on
# the machine.
XOS_MODEL_PATH=/opt/xos/models/current.gguf
XOS_CTX_SIZE=8192
# 999 means every layer on the GPU. 0 means none, for a CPU-only machine.
XOS_GPU_LAYERS=999
ENV

xlog ""
xlog "   Anything that failed above leaves XOS working through an API."
exit 0
