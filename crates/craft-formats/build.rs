// Builds the OpenCascade shim when the `occt` feature is on. OCCT static libraries are taken from
// $OCCT_DIR (an OCCT 7.8 build tree or install: include/ + lib/).
fn main() {
    println!("cargo:rerun-if-changed=occt/shim.cpp");
    println!("cargo:rerun-if-env-changed=OCCT_DIR");
    println!("cargo:rerun-if-env-changed=OCCT_INC");
    println!("cargo:rerun-if-env-changed=OCCT_LIB");
    #[cfg(feature = "occt")]
    occt();
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
    b.cpp(true).file("occt/shim.cpp").include(&inc).flag_if_supported("-std=c++17").flag_if_supported("-w").define("OCCT_STATIC", None);
    if windows {
        b.define("HAVE_NO_DLL", None);
    }
    b.compile("occtshim");
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
