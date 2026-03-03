use std::{env, path::PathBuf};

fn main() {
    // 获取当前编译的目标操作系统
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // 只有编译 windows 目标时才添加构建与链接指令
    if target_os == "windows" {
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set");

        // 构建到 ../libs/winpty-0.4.3-msys2-2.7.0-x64/lib 目录的绝对/相对路径
        let lib_path = PathBuf::from(manifest_dir).join("../libs/winpty-0.4.3-msys2-2.7.0-x64/lib");

        // 告诉 Cargo 库文件的搜索路径
        println!("cargo:rustc-link-search=native={}", lib_path.display());

        // 告诉 Cargo 链接 winpty (即 winpty.lib)
        println!("cargo:rustc-link-lib=dylib=winpty");
    }
}
