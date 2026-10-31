// 嵌入 assets/app.ico 作为 exe 图标（任务栏 / 资源管理器 / 托盘加载源）。
// 直接调用 Windows SDK 的 rc.exe 编译 .res 并链接；找不到 rc.exe 或编译失败
// 时只告警不阻断——托盘会回退到运行时绘制图标。
use std::path::{Path, PathBuf};

fn find_rc_exe() -> Option<PathBuf> {
    // 新版本优先：Windows Kits\10\bin\<ver>\x64\rc.exe
    let root = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    if let Ok(entries) = std::fs::read_dir(root) {
        let mut versions: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        versions.sort();
        versions.reverse();
        for version in versions {
            let candidate = version.join("x64").join("rc.exe");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    let manifest = Path::new(&manifest_dir);
    let icon = manifest.join("assets").join("app.ico");
    if !icon.exists() {
        println!("cargo:warning=未找到 assets/app.ico，跳过资源嵌入");
        return;
    }
    let Some(rc_exe) = find_rc_exe() else {
        println!("cargo:warning=未找到 Windows SDK rc.exe，跳过资源嵌入（托盘回退运行时图标）");
        return;
    };

    // rc 的字符串路径用正斜杠避免转义问题
    let icon_str = icon.to_string_lossy().replace('\\', "/");
    let rc_source = manifest.join("target").join("app.rc");
    if let Some(parent) = rc_source.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // 资源：图标 + 应用清单（comctl32 v6 视觉样式 + PerMonitorV2 DPI）
    let manifest_str = manifest
        .join("assets")
        .join("app.manifest")
        .to_string_lossy()
        .replace('\\', "/");
    let rc_text = if Path::new(&manifest_str).exists() {
        format!("1 ICON \"{icon_str}\"\n1 24 \"{manifest_str}\"\n")
    } else {
        format!("1 ICON \"{icon_str}\"\n")
    };
    std::fs::write(&rc_source, rc_text).expect("写 app.rc");

    let res_path = manifest.join("target").join("app.res");
    let status = std::process::Command::new(&rc_exe)
        .args([
            "/nologo",
            &format!("/fo{}", res_path.to_string_lossy()),
            rc_source.to_string_lossy().as_ref(),
        ])
        .status();
    match status {
        Ok(status) if status.success() => {
            println!(
                "cargo:rustc-link-arg-bins={}",
                res_path.to_string_lossy()
            );
        }
        Ok(status) => {
            println!(
                "cargo:warning=rc.exe 退出码 {}，跳过资源嵌入（托盘回退运行时图标）",
                status.code().unwrap_or(-1)
            );
        }
        Err(err) => {
            println!("cargo:warning=rc.exe 执行失败，跳过资源嵌入: {err}");
        }
    }
}
