// Builds the OpenCascade shim when the `occt` feature is on. OCCT static libraries are taken from
// $OCCT_DIR (an OCCT 7.8 build tree or install: include/ + lib/).
fn main() {
    println!("cargo:rerun-if-changed=occt/shim.cpp");
    println!("cargo:rerun-if-env-changed=OCCT_DIR");
    println!("cargo:rerun-if-env-changed=OCCT_INC");
    println!("cargo:rerun-if-env-changed=OCCT_LIB");
    println!("cargo:rerun-if-changed=assimp/shim.cpp");
    println!("cargo:rerun-if-env-changed=ASSIMP_DIR");
    #[cfg(feature = "occt")]
    occt();
    #[cfg(feature = "assimp")]
    assimp();
    println!("cargo:rerun-if-changed=webifc/shim.cpp");
    println!("cargo:rerun-if-env-changed=WEBIFC_SRC");
    println!("cargo:rerun-if-env-changed=WEBIFC_BUILD");
    #[cfg(feature = "ifc")]
    webifc();
}

/// web-ifc (MPL-2.0) static library: sources in $WEBIFC_SRC (src/cpp of the repository), CMake
/// build tree in $WEBIFC_BUILD (holds libweb-ifc-library.a and the fetched header-only deps).
#[cfg(feature = "ifc")]
fn webifc() {
    use std::path::PathBuf;
    let target = std::env::var("TARGET").unwrap();
    let windows = target.contains("windows");
    let src = std::env::var("WEBIFC_SRC").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/workspace/deps/web-ifc/src/cpp"));
    let build = std::env::var("WEBIFC_BUILD").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(if windows { "/workspace/deps/wi-mingw" } else { "/workspace/deps/wi-linux" }));
    // the fetched deps live in the Linux tree (the mingw configure reused them)
    let deps = PathBuf::from("/workspace/deps/wi-linux/_deps");
    let deps = if build.join("_deps/glm-src").exists() { build.join("_deps") } else { deps };
    let mut b = cc::Build::new();
    b.cpp(true).file("webifc/shim.cpp").include(&src).include(src.join("web-ifc")).flag_if_supported("-std=c++20").flag_if_supported("-w").define("mapbox", "wi_mapbox");
    for d in ["glm-src", "glm-src/glm", "tinynurbs-src/include", "fastfloat-src/include", "cdt-src/CDT/include", "earcut-src/include", "spdlog-src/include", "stduuid-src/include", "unordered_dense-src/include"] {
        b.include(deps.join(d));
    }
    b.compile("webifcshim");
    println!("cargo:rustc-link-search=native={}", build.display());
    println!("cargo:rustc-link-lib=static=web-ifc-library");
    if !windows {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }
}

/// Static assimp 6 (importers only, bundled zlib) from $ASSIMP_DIR (an install prefix: include/ + lib/).
#[cfg(feature = "assimp")]
fn assimp() {
    use std::path::PathBuf;
    let target = std::env::var("TARGET").unwrap();
    let windows = target.contains("windows");
    let default = if windows { "/workspace/deps/assimp-mingw" } else { "/workspace/deps/assimp-linux" };
    let dir = std::env::var("ASSIMP_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(default));
    let mut b = cc::Build::new();
    b.cpp(true).file("assimp/shim.cpp").include(dir.join("include")).flag_if_supported("-std=gnu++17").flag_if_supported("-w");
    let cxx = b.get_compiler().path().to_path_buf();
    b.compile("assimpshim");
    println!("cargo:rustc-link-search=native={}", dir.join("lib").display());
    println!("cargo:rustc-link-lib=static=assimp");
    println!("cargo:rustc-link-lib=static=zlibstatic");
    if windows {
        for f in ["libstdc++.a", "libwinpthread.a"] {
            if let Ok(o) = std::process::Command::new(&cxx).arg(format!("-print-file-name={f}")).output() {
                let p = PathBuf::from(String::from_utf8_lossy(&o.stdout).trim());
                if let Some(d) = p.parent().filter(|_| p.is_absolute()) {
                    println!("cargo:rustc-link-search=native={}", d.display());
                }
            }
        }
        for l in ["stdc++", "winpthread"] {
            println!("cargo:rustc-link-lib=static={l}");
        }
    } else {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }
}

#[cfg(feature = "occt")]
fn occt() {
    use std::path::PathBuf;
    let dir = std::env::var("OCCT_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/workspace/occt/install"));
    let inc = std::env::var("OCCT_INC").map(PathBuf::from).unwrap_or_else(|_| dir.join("include/opencascade"));
    let lib = std::env::var("OCCT_LIB").map(PathBuf::from).unwrap_or_else(|_| dir.join("lib"));
    let target = std::env::var("TARGET").unwrap();
    let windows = target.contains("windows");
    let mut b = cc::Build::new();
    b.cpp(true).file("occt/shim.cpp").include(&inc).flag_if_supported("-std=gnu++17").define("_USE_MATH_DEFINES", None).flag_if_supported("-w").define("OCCT_STATIC", None);
    if windows {
        b.define("HAVE_NO_DLL", None);
    }
    let cxx = b.get_compiler().path().to_path_buf();
    b.compile("occtshim");
    if windows {
        // static libstdc++/winpthread live next to the cross compiler, not in the default search path
        for f in ["libstdc++.a", "libwinpthread.a"] {
            if let Ok(o) = std::process::Command::new(&cxx).arg(format!("-print-file-name={f}")).output() {
                let p = PathBuf::from(String::from_utf8_lossy(&o.stdout).trim());
                if let Some(d) = p.parent().filter(|_| p.is_absolute()) {
                    println!("cargo:rustc-link-search=native={}", d.display());
                }
            }
        }
    }
    println!("cargo:rustc-link-search=native={}", lib.display());
    // dependency order: users first
    let libs = [
        "TKDESTEP", "TKDEIGES", "TKXCAF", "TKVCAF", "TKV3d", "TKService", "TKHLR", "TKXSBase", "TKDE", "TKCAF", "TKLCAF", "TKCDF", "TKMesh",
        "TKBool", "TKBO", "TKPrim", "TKShHealing", "TKTopAlgo", "TKGeomAlgo", "TKBRep", "TKGeomBase", "TKG3d", "TKG2d", "TKMath", "TKernel",
    ];
    for l in libs {
        println!("cargo:rustc-link-lib=static={l}");
    }
    if windows {
        for l in ["stdc++", "winpthread"] {
            println!("cargo:rustc-link-lib=static={l}");
        }
        for l in ["user32", "advapi32", "gdi32", "ws2_32", "psapi", "shell32", "ole32", "oleaut32", "uuid", "comdlg32", "winspool"] {
            println!("cargo:rustc-link-lib=dylib={l}");
        }
    } else {
        println!("cargo:rustc-link-lib=dylib=stdc++");
        println!("cargo:rustc-link-lib=dylib=pthread");
        println!("cargo:rustc-link-lib=dylib=dl");
    }
}
