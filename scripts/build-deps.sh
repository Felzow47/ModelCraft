#!/usr/bin/env bash
# Build the static native libraries of the Windows release (cross-compiled on Linux with
# mingw-w64 "posix"): OpenCascade 7.8.1 (STEP/IGES), assimp 6.0.2 (FBX, glTF…) and web-ifc (IFC).
#
#   scripts/build-deps.sh [DEPS_DIR]      (default: ./deps)
#
# Needs: cmake, ninja, git, mingw-w64 (x86_64-w64-mingw32-gcc-posix / g++-posix).
# Writes DEPS_DIR/env.sh, to be sourced before `cargo build --target x86_64-pc-windows-gnu`.
set -euo pipefail

DEPS=$(mkdir -p "${1:-deps}" && cd "${1:-deps}" && pwd)
JOBS=$(nproc)
OCCT_TAG=V7_8_1
ASSIMP_TAG=v6.0.2
WEBIFC_COMMIT=6b33ddf26c74d4881febaabf7a5bcb5259eccd96

TC="$DEPS/mingw.cmake"
cat > "$TC" <<'EOF'
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR x86_64)
set(CMAKE_C_COMPILER x86_64-w64-mingw32-gcc-posix)
set(CMAKE_CXX_COMPILER x86_64-w64-mingw32-g++-posix)
set(CMAKE_RC_COMPILER x86_64-w64-mingw32-windres)
set(CMAKE_FIND_ROOT_PATH /usr/x86_64-w64-mingw32)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
EOF

fetch() { # url ref dir
    [ -d "$3/.git" ] && return
    git init -q "$3"
    git -C "$3" fetch -q --depth 1 "$1" "$2"
    git -C "$3" checkout -q FETCH_HEAD
}

if [ ! -f "$DEPS/occt/lib/libTKernel.a" ]; then
    fetch https://github.com/Open-Cascade-SAS/OCCT.git "$OCCT_TAG" "$DEPS/src/occt"
    cmake -S "$DEPS/src/occt" -B "$DEPS/build/occt" -G Ninja -DCMAKE_TOOLCHAIN_FILE="$TC" \
        -DCMAKE_BUILD_TYPE=Release -DBUILD_LIBRARY_TYPE=Static -DINSTALL_DIR="$DEPS/occt" -DINSTALL_DIR_LAYOUT=Unix \
        -DBUILD_MODULE_Draw=OFF -DBUILD_MODULE_DETools=OFF -DBUILD_MODULE_ApplicationFramework=ON -DBUILD_MODULE_DataExchange=ON \
        -DBUILD_MODULE_Visualization=ON -DBUILD_MODULE_ModelingAlgorithms=ON -DBUILD_MODULE_ModelingData=ON \
        -DBUILD_DOC_Overview=OFF -DBUILD_SAMPLES_QT=OFF -DUSE_FREETYPE=OFF -DUSE_OPENGL=OFF -DUSE_GLES2=OFF \
        -DUSE_D3D=OFF -DUSE_TK=OFF -DUSE_TCL=OFF -DUSE_FREEIMAGE=OFF -DUSE_RAPIDJSON=OFF -DUSE_DRACO=OFF \
        -DUSE_TBB=OFF -DUSE_VTK=OFF -DUSE_FFMPEG=OFF -DUSE_OPENVR=OFF
    cmake --build "$DEPS/build/occt" -j "$JOBS"
    cmake --install "$DEPS/build/occt"
fi

if [ ! -f "$DEPS/assimp/lib/libassimp.a" ]; then
    fetch https://github.com/assimp/assimp.git "$ASSIMP_TAG" "$DEPS/src/assimp"
    cmake -S "$DEPS/src/assimp" -B "$DEPS/build/assimp" -G Ninja -DCMAKE_TOOLCHAIN_FILE="$TC" \
        -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$DEPS/assimp" -DBUILD_SHARED_LIBS=OFF \
        -DASSIMP_BUILD_VRML_IMPORTER=OFF -DASSIMP_BUILD_USD_IMPORTER=ON -DASSIMP_BUILD_TESTS=OFF \
        -DASSIMP_BUILD_ASSIMP_TOOLS=OFF -DASSIMP_BUILD_SAMPLES=OFF -DASSIMP_NO_EXPORT=ON \
        -DASSIMP_BUILD_ZLIB=ON -DASSIMP_WARNINGS_AS_ERRORS=OFF -DASSIMP_INSTALL_PDB=OFF
    cmake --build "$DEPS/build/assimp" -j "$JOBS"
    cmake --install "$DEPS/build/assimp"
    # the static zlib is built with a "d"/no suffix depending on the generator: keep the name build.rs links
    [ -f "$DEPS/assimp/lib/libzlibstatic.a" ] || cp "$(find "$DEPS/build/assimp" -name 'libzlibstatic*.a' | head -1)" "$DEPS/assimp/lib/libzlibstatic.a"
    [ -f "$DEPS/assimp/lib/libassimp.a" ] || cp "$(find "$DEPS/assimp/lib" -name 'libassimp*.a' | head -1)" "$DEPS/assimp/lib/libassimp.a"
fi

if [ ! -f "$DEPS/web-ifc/build/libweb-ifc-library.a" ]; then
    fetch https://github.com/ThatOpen/engine_web-ifc.git "$WEBIFC_COMMIT" "$DEPS/web-ifc/repo"
    cmake -S "$DEPS/web-ifc/repo/src/cpp" -B "$DEPS/web-ifc/build" -G Ninja -DCMAKE_TOOLCHAIN_FILE="$TC" \
        -DCMAKE_BUILD_TYPE=Release -DCMAKE_CXX_FLAGS="-Dmapbox=wi_mapbox"
    cmake --build "$DEPS/web-ifc/build" -j "$JOBS" --target web-ifc-library
    lib=$(find "$DEPS/web-ifc/build" -name 'libweb-ifc-library.a' | head -1)
    [ "$lib" = "$DEPS/web-ifc/build/libweb-ifc-library.a" ] || cp "$lib" "$DEPS/web-ifc/build/"
fi

cat > "$DEPS/env.sh" <<EOF
export CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++-posix
export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc-posix
export OCCT_INC="$DEPS/occt/include/opencascade"
export OCCT_LIB="$DEPS/occt/lib"
export ASSIMP_DIR="$DEPS/assimp"
export WEBIFC_SRC="$DEPS/web-ifc/repo/src/cpp"
export WEBIFC_BUILD="$DEPS/web-ifc/build"
EOF
echo "deps ready in $DEPS"
