#[cfg(windows)]
use std::fs;
#[cfg(windows)]
use std::path::Path;

#[cfg(windows)]
#[derive(rust_embed::RustEmbed)]
#[folder = "../libs/winpty-0.4.3-msys2-2.7.0-x64/bin/"]
struct WinptyAssets;

#[cfg(windows)]
#[derive(rust_embed::RustEmbed)]
#[folder = "../libs/clink/"]
struct ClinkAssets;

#[cfg(windows)]
pub fn install_deps() -> anyhow::Result<()> {
    // Install winpty
    let winpty_dir = Path::new("winpty");
    for file in WinptyAssets::iter() {
        let f = file.as_ref();
        let content = WinptyAssets::get(f).unwrap();
        let target_path = winpty_dir.join(f);
        if let Some(p) = target_path.parent() {
            fs::create_dir_all(p)?;
        }
        fs::write(target_path, content.data)?;
    }
    println!("WinPTY exported successfully.");

    // Install clink
    let clink_dir = Path::new("clink");
    for file in ClinkAssets::iter() {
        let f = file.as_ref();
        let content = ClinkAssets::get(f).unwrap();
        let target_path = clink_dir.join(f);
        if let Some(p) = target_path.parent() {
            fs::create_dir_all(p)?;
        }
        fs::write(target_path, content.data)?;
    }
    println!("Clink exported successfully.");

    println!("Dependencies installed successfully.");
    Ok(())
}

#[cfg(not(windows))]
pub fn install_deps() -> anyhow::Result<()> {
    println!("Dependencies installation is only required and supported on Windows.");
    Ok(())
}
