//! `cargo xtask <command>` — build and packaging automation (PLAN §8.4, §13).
//! Runs inside the `glidedesk-builder` container locally and directly on the
//! GitHub runners in CI. Output goes to `output-build/`.
//!
//! Commands:
//!   ui                         build the web UI (pnpm)
//!   check-cross <targets…>     clippy for each cross target
//!   package macos              Apple Silicon Glidedesk.app + .dmg (+ uninstaller)
//!   package windows            x64 NSIS installers (standard + offline WebView2)
//!   package linux              .deb / .rpm / .tar.gz for this machine's architecture
//!   checksums                  SHA256SUMS (+ minisign signature when a key is set)
//!   release                    everything this host can build + checksums
//!
//! Signing (all optional; see scripts/generate-signing-keys.sh):
//!   GLIDEDESK_MAC_P12 + GLIDEDESK_MAC_P12_PASSWORD     self-signed macOS identity
//!   GLIDEDESK_WIN_PFX + GLIDEDESK_WIN_PFX_PASSWORD     self-signed Authenticode cert
//!   GLIDEDESK_MINISIGN_KEY                             minisign secret key (no password)

#![allow(clippy::doc_markdown)] // env var names in docs

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

type Result<T = ()> = std::result::Result<T, String>;

const APP_ID: &str = "app.glidedesk.desktop";
const PRODUCT: &str = "Glidedesk";
const MAC_TARGET: &str = "aarch64-apple-darwin";
const WIN_TARGET: &str = "x86_64-pc-windows-msvc";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

fn out_dir() -> PathBuf {
    root().join("output-build")
}

fn target_dir() -> PathBuf {
    env::var_os("CARGO_TARGET_DIR").map_or_else(|| root().join("target"), PathBuf::from)
}

fn cache_dir() -> PathBuf {
    env::var_os("GLIDEDESK_CACHE").map_or_else(|| target_dir().join("gd-cache"), PathBuf::from)
}

fn version() -> String {
    if let Ok(v) = env::var("GLIDEDESK_VERSION")
        && !v.is_empty()
    {
        return v.trim_start_matches('v').to_owned();
    }
    let toml = fs::read_to_string(root().join("Cargo.toml")).unwrap_or_default();
    toml.lines()
        .skip_while(|l| !l.starts_with("[workspace.package]"))
        .find_map(|l| l.strip_prefix("version = ").map(|v| v.trim_matches('"').to_owned()))
        .unwrap_or_else(|| "0.0.0".into())
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name).map(PathBuf::from).filter(|p| p.is_file())
}

fn run(cmd: &mut Command) -> Result {
    eprintln!("→ {}", cmd.get_program().to_string_lossy());
    let status = cmd.status().map_err(|e| format!("cannot run {cmd:?}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("command failed ({status}): {}", cmd.get_program().to_string_lossy()))
    }
}

fn sh(program: &str) -> Command {
    let mut c = Command::new(program);
    c.current_dir(root());
    c
}

fn copy(from: &Path, to: &Path) -> Result {
    if let Some(p) = to.parent() {
        fs::create_dir_all(p).map_err(|e| format!("mkdir {}: {e}", p.display()))?;
    }
    fs::copy(from, to).map(|_| ()).map_err(|e| format!("copy {} → {}: {e}", from.display(), to.display()))
}

fn write(path: &Path, text: &str) -> Result {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).map_err(|e| format!("mkdir {}: {e}", p.display()))?;
    }
    fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

fn set_exec(p: &Path) -> Result {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(p, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())
}

fn clean_dir(p: &Path) -> Result {
    if p.exists() {
        fs::remove_dir_all(p).map_err(|e| format!("rm {}: {e}", p.display()))?;
    }
    fs::create_dir_all(p).map_err(|e| format!("mkdir {}: {e}", p.display()))
}

// ---------------------------------------------------------------------------

fn build_ui() -> Result {
    let ui = root().join("app/ui");
    run(Command::new("pnpm").current_dir(&ui).args(["install", "--frozen-lockfile"]))?;
    run(Command::new("pnpm").current_dir(&ui).arg("build"))?;
    if !ui.join("dist/index.html").is_file() {
        return Err("UI build produced no dist/index.html".into());
    }
    Ok(())
}

fn check_cross(targets: &[String]) -> Result {
    for t in targets {
        let mut c = sh("cargo");
        if t.contains("windows") {
            c.arg("xwin");
        }
        run(c.args(["clippy", "--workspace", "--exclude", "xtask", "--target", t, "--", "-D", "warnings"]))?;
    }
    Ok(())
}

/// Release build of the single `glidedesk` executable (the agent is a library inside it).
fn cargo_build(target: Option<&str>) -> Result<PathBuf> {
    let mut c = sh("cargo");
    if target.is_some_and(|t| t.contains("windows")) {
        c.arg("xwin");
    }
    c.args(["build", "--release", "--locked", "-p", "glidedesk-app", "--features", "glidedesk-app/custom-protocol"]);
    if let Some(t) = target {
        c.args(["--target", t]);
    }
    run(&mut c)?;
    let dir = match target {
        Some(t) => target_dir().join(t).join("release"),
        None => target_dir().join("release"),
    };
    let exe = dir.join(if target.is_some_and(|t| t.contains("windows")) { "glidedesk.exe" } else { "glidedesk" });
    if !exe.is_file() {
        return Err(format!("build produced no {}", exe.display()));
    }
    Ok(exe)
}

// ---------------------------------------------------------------------------
// signing
// ---------------------------------------------------------------------------

/// macOS: stable self-signed identity when configured, otherwise ad-hoc.
fn mac_sign(path: &Path, identifier: &str) -> Result {
    let mut c = Command::new("rcodesign");
    c.args(["sign", "--binary-identifier", identifier, "--code-signature-flags", "runtime"]);
    match (env_path("GLIDEDESK_MAC_P12"), env::var("GLIDEDESK_MAC_P12_PASSWORD")) {
        (Some(p12), Ok(pw)) => {
            c.arg("--p12-file").arg(p12).arg("--p12-password").arg(pw);
        }
        _ => eprintln!("  (no GLIDEDESK_MAC_P12: ad-hoc signature)"),
    }
    run(c.arg(path))
}

fn win_signing_enabled() -> bool {
    env_path("GLIDEDESK_WIN_PFX").is_some() && env::var("GLIDEDESK_WIN_PFX_PASSWORD").is_ok()
}

/// Windows: Authenticode with the self-signed certificate (in place).
fn win_sign(path: &Path) -> Result {
    if !win_signing_enabled() {
        eprintln!("  (no GLIDEDESK_WIN_PFX: {} left unsigned)", path.display());
        return Ok(());
    }
    run(Command::new("gd-sign-windows").arg(path))
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

fn info_plist(version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleDisplayName</key><string>{PRODUCT}</string>
  <key>CFBundleExecutable</key><string>glidedesk</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>CFBundleIdentifier</key><string>{APP_ID}</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>{PRODUCT}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
  <key>CFBundleVersion</key><string>{version}</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
  <key>LSArchitecturePriority</key><array><string>arm64</string></array>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSLocalNetworkUsageDescription</key>
  <string>Glidedesk connects to your other computers on the local network to share the keyboard, mouse and clipboard.</string>
  <key>NSBonjourServices</key>
  <array><string>_glidedesk._udp</string></array>
  <key>NSHumanReadableCopyright</key><string>Private software.</string>
</dict>
</plist>
"#
    )
}

fn uninstaller_plist(version: &str) -> String {
    info_plist(version)
        .replace("<string>glidedesk</string>", "<string>uninstall</string>")
        .replace(APP_ID, "app.glidedesk.uninstaller")
        .replace("<key>LSUIElement</key><true/>", "<key>LSUIElement</key><false/>")
        .replace(
            &format!("<key>CFBundleDisplayName</key><string>{PRODUCT}</string>"),
            "<key>CFBundleDisplayName</key><string>Uninstall Glidedesk</string>",
        )
}

fn package_macos() -> Result {
    let on_mac = cfg!(target_os = "macos");
    if !on_mac {
        let sdk = env::var("MACOS_SDK").unwrap_or_else(|_| "/opt/macos-sdk".into());
        if !Path::new(&sdk).join("usr/lib").exists() {
            return Err("macOS SDK is not mounted — run `make sdk` on the Mac first".into());
        }
    }
    build_ui()?;
    let exe = cargo_build(Some(MAC_TARGET))?;
    let v = version();
    let stage = target_dir().join("stage/macos");
    clean_dir(&stage)?;
    let app = stage.join(format!("{PRODUCT}.app"));
    let contents = app.join("Contents");
    copy(&exe, &contents.join("MacOS/glidedesk"))?;
    set_exec(&contents.join("MacOS/glidedesk"))?;
    copy(&root().join("app/icons/icon.icns"), &contents.join("Resources/icon.icns"))?;
    copy(&root().join("installer/macos/uninstall.sh"), &contents.join("Resources/uninstall.sh"))?;
    set_exec(&contents.join("Resources/uninstall.sh"))?;
    write(&contents.join("Info.plist"), &info_plist(&v))?;
    write(&contents.join("PkgInfo"), "APPL????")?;
    mac_sign(&app, APP_ID)?;

    // "Uninstall Glidedesk.app": a tiny bundle that runs the same script.
    let un = stage.join("Uninstall Glidedesk.app/Contents");
    copy(&root().join("installer/macos/uninstall.sh"), &un.join("MacOS/uninstall"))?;
    set_exec(&un.join("MacOS/uninstall"))?;
    copy(&root().join("app/icons/icon.icns"), &un.join("Resources/icon.icns"))?;
    write(&un.join("Info.plist"), &uninstaller_plist(&v))?;

    std::os::unix::fs::symlink("/Applications", stage.join("Applications")).map_err(|e| e.to_string())?;
    let out = out_dir();
    fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let dmg = out.join(format!("{PRODUCT}_{v}_macos-arm64.dmg"));
    let _ = fs::remove_file(&dmg);
    if on_mac {
        run(Command::new("hdiutil")
            .args(["create", "-volname", PRODUCT, "-fs", "HFS+", "-format", "UDZO", "-srcfolder"])
            .arg(&stage)
            .arg(&dmg))?;
    } else {
        let iso = target_dir().join("stage/glidedesk.iso");
        run(Command::new("xorrisofs")
            .args(["-D", "-l", "-V", PRODUCT, "-no-pad", "-r", "-dir-mode", "0755", "-o"])
            .arg(&iso)
            .arg(&stage))?;
        run(Command::new("dmg").arg(&iso).arg(&dmg))?;
    }
    let tgz = out.join(format!("{PRODUCT}_{v}_macos-arm64.app.tar.gz"));
    run(Command::new("tar").current_dir(&stage).arg("-czf").arg(&tgz).arg(format!("{PRODUCT}.app")))?;
    eprintln!("✓ {}\n✓ {}", dmg.display(), tgz.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// Downloads (once, cached) the `WebView2` bootstrapper or the x64 offline runtime.
fn webview2(offline: bool) -> Result<PathBuf> {
    let dir = cache_dir().join("webview2");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (name, url) = if offline {
        ("MicrosoftEdgeWebView2RuntimeInstallerX64.exe", "https://go.microsoft.com/fwlink/?linkid=2124701")
    } else {
        ("MicrosoftEdgeWebview2Setup.exe", "https://go.microsoft.com/fwlink/p/?LinkId=2124703")
    };
    let path = dir.join(name);
    if !path.exists() {
        let tmp = dir.join(format!("{name}.part"));
        run(Command::new("curl").args(["-fsSL", "--retry", "3", "-o"]).arg(&tmp).arg(url))?;
        fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    }
    Ok(path)
}

fn package_windows() -> Result {
    build_ui()?;
    let v = version();
    let out = out_dir();
    fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let exe = cargo_build(Some(WIN_TARGET))?;
    // Sign the program itself before it is packed into the installer.
    let signed = target_dir().join("stage/windows/glidedesk.exe");
    copy(&exe, &signed)?;
    win_sign(&signed)?;
    let nsi = root().join("installer/windows/glidedesk.nsi");
    for offline in [false, true] {
        let wv = webview2(offline)?;
        let suffix = if offline { "-offline" } else { "" };
        let setup = out.join(format!("{PRODUCT}_{v}_windows-x64{suffix}-setup.exe"));
        let mut c = sh("makensis");
        c.args(["-V2", "-INPUTCHARSET", "UTF8"])
            .arg(format!("-DVERSION={v}"))
            .arg("-DARCH=x64")
            .arg(format!("-DAPP_EXE={}", signed.display()))
            .arg(format!("-DICON={}", root().join("app/icons/icon.ico").display()))
            .arg(format!("-DWEBVIEW2={}", wv.display()))
            .arg(format!("-DWEBVIEW2_OFFLINE={}", u8::from(offline)))
            .arg(format!("-DOUTFILE={}", setup.display()));
        if win_signing_enabled() {
            // Signs the uninstaller and the finished installer (NSIS !finalize hooks).
            c.arg("-DSIGN=1");
        }
        run(c.arg(&nsi))?;
        eprintln!("✓ {}", setup.display());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------

fn linux_arch() -> Result<(&'static str, &'static str, &'static str)> {
    // (our name, deb arch, rpm arch)
    match env::consts::ARCH {
        "x86_64" => Ok(("x64", "amd64", "x86_64")),
        "aarch64" => Ok(("arm64", "arm64", "aarch64")),
        other => Err(format!("unsupported Linux architecture {other}")),
    }
}

fn nfpm_config(v: &str, arch: &str, stage: &Path) -> String {
    let s = stage.display();
    format!(
        r"name: glidedesk
arch: {arch}
platform: linux
version: {v}
section: utils
priority: optional
maintainer: Glidedesk <noreply@glidedesk.invalid>
description: |
  One keyboard and mouse for your computers. Move the cursor off the edge of
  the screen and it continues on the next computer; the clipboard and copied
  files follow it.
homepage: https://glidedesk.invalid
license: Proprietary
contents:
  - src: {s}/glidedesk
    dst: /usr/bin/glidedesk
    file_info: {{ mode: 0755 }}
  - src: {s}/glidedesk.desktop
    dst: /usr/share/applications/glidedesk.desktop
  - src: {s}/icons/32.png
    dst: /usr/share/icons/hicolor/32x32/apps/glidedesk.png
  - src: {s}/icons/128.png
    dst: /usr/share/icons/hicolor/128x128/apps/glidedesk.png
  - src: {s}/icons/256.png
    dst: /usr/share/icons/hicolor/256x256/apps/glidedesk.png
  - src: {s}/icons/512.png
    dst: /usr/share/icons/hicolor/512x512/apps/glidedesk.png
  - src: {s}/70-glidedesk-uinput.rules
    dst: /usr/lib/udev/rules.d/70-glidedesk-uinput.rules
  - src: {s}/glidedesk-uinput.conf
    dst: /usr/lib/modules-load.d/glidedesk-uinput.conf
scripts:
  postinstall: {s}/postinstall.sh
  postremove: {s}/postremove.sh
overrides:
  deb:
    depends: [libwebkit2gtk-4.1-0, libgtk-3-0, libayatana-appindicator3-1, libxdo3]
  rpm:
    depends: [webkit2gtk4.1, gtk3, libayatana-appindicator-gtk3, libxdo]
"
    )
}

fn package_linux() -> Result {
    build_ui()?;
    let exe = cargo_build(None)?;
    let (name, deb_arch, rpm_arch) = linux_arch()?;
    let v = version();
    let stage = target_dir().join(format!("stage/linux-{name}"));
    clean_dir(&stage)?;
    let li = root().join("installer/linux");
    copy(&exe, &stage.join("glidedesk"))?;
    set_exec(&stage.join("glidedesk"))?;
    for f in
        ["glidedesk.desktop", "70-glidedesk-uinput.rules", "glidedesk-uinput.conf", "postinstall.sh", "postremove.sh"]
    {
        copy(&li.join(f), &stage.join(f))?;
    }
    set_exec(&stage.join("postinstall.sh"))?;
    set_exec(&stage.join("postremove.sh"))?;
    for (px, file) in [("32", "32x32.png"), ("128", "128x128.png"), ("256", "256x256.png"), ("512", "512x512.png")] {
        copy(&root().join("app/icons").join(file), &stage.join(format!("icons/{px}.png")))?;
    }
    let out = out_dir();
    fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    for (packager, arch, ext) in [("deb", deb_arch, "deb"), ("rpm", rpm_arch, "rpm")] {
        let cfg = stage.join(format!("nfpm-{packager}.yaml"));
        write(&cfg, &nfpm_config(&v, arch, &stage))?;
        let target = out.join(format!("{PRODUCT}_{v}_linux-{name}.{ext}"));
        run(Command::new("nfpm")
            .args(["package", "--packager", packager, "--config"])
            .arg(&cfg)
            .arg("--target")
            .arg(&target))?;
        eprintln!("✓ {}", target.display());
    }
    // Portable archive: the same files plus a small installer script.
    copy(&li.join("install.sh"), &stage.join("install.sh"))?;
    set_exec(&stage.join("install.sh"))?;
    let tgz = out.join(format!("{PRODUCT}_{v}_linux-{name}.tar.gz"));
    run(Command::new("tar").current_dir(&stage).arg("-czf").arg(&tgz).args([
        "glidedesk",
        "glidedesk.desktop",
        "icons",
        "70-glidedesk-uinput.rules",
        "glidedesk-uinput.conf",
        "install.sh",
    ]))?;
    eprintln!("✓ {}", tgz.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// checksums
// ---------------------------------------------------------------------------

fn checksums() -> Result {
    let out = out_dir();
    let mut entries: Vec<PathBuf> = fs::read_dir(&out)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && p.file_name().is_some_and(|n| {
                    let n = n.to_string_lossy();
                    !n.starts_with('.') && !n.starts_with("SHA256SUMS")
                })
        })
        .collect();
    entries.sort();
    if entries.is_empty() {
        return Err("output-build/ is empty".into());
    }
    let mut text = String::new();
    for p in entries {
        let o = Command::new("sha256sum").arg(&p).output().map_err(|e| e.to_string())?;
        if !o.status.success() {
            return Err(format!("sha256sum failed for {}: {}", p.display(), String::from_utf8_lossy(&o.stderr)));
        }
        let line = String::from_utf8_lossy(&o.stdout);
        let hash = line.split_whitespace().next().filter(|h| h.len() == 64).ok_or("sha256sum gave no hash")?;
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let _ = writeln!(text, "{hash}  {name}");
    }
    let sums = out.join("SHA256SUMS");
    write(&sums, &text)?;
    if let Some(key) = env_path("GLIDEDESK_MINISIGN_KEY") {
        run(Command::new("minisign")
            .args(["-S", "-s"])
            .arg(key)
            .arg("-m")
            .arg(&sums)
            .args(["-t", "Glidedesk release checksums"]))?;
        eprintln!("✓ SHA256SUMS.minisig");
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let res = match args.first().map(String::as_str) {
        Some("ui") => build_ui(),
        Some("check-cross") => check_cross(&args[1..]),
        Some("package") => match args.get(1).map(String::as_str) {
            Some("macos") => package_macos(),
            Some("windows") => package_windows(),
            Some("linux") => package_linux(),
            _ => Err("usage: cargo xtask package <macos|windows|linux>".into()),
        },
        Some("release") => {
            package_macos().and_then(|()| package_windows()).and_then(|()| package_linux()).and_then(|()| checksums())
        }
        Some("checksums") => checksums(),
        _ => Err("usage: cargo xtask <ui|check-cross|package|release|checksums>".into()),
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
