//! The standalone esbuild and Tailwind binaries `new` downloads, so a project needs no
//! Node. Every download is pinned to a version and a SHA-256 and is checked before it
//! is unpacked or run; a mismatch installs nothing. Verified tools are cached per user.

use crate::Error;
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const ESBUILD: &str = "0.28.2";
const TAILWIND: &str = "4.3.3";

/// One pinned download.
#[derive(Debug, Clone)]
pub struct Pin {
    pub name: &'static str,
    pub version: &'static str,
    pub url: String,
    /// Lower-case hex SHA-256 of the file at `url`.
    pub sha256: &'static str,
    /// The executable's path inside a `.tgz`; `None` when the download is the executable.
    pub member: Option<&'static str>,
}

/// The pins for the platform `os`/`arch` (as `std::env::consts` spells them).
pub fn pins(os: &str, arch: &str) -> Result<[Pin; 2], Error> {
    // (esbuild npm package, esbuild sha256 of its tarball, tailwind asset, its sha256)
    let (package, esbuild, asset, tailwind) = match (os, arch) {
        ("macos", "aarch64") => (
            "darwin-arm64",
            "1980cde09749094452b20d36ff267585ccb3f72749c7bc97291cd9996ccf5a2a",
            "macos-arm64",
            "cdf646702987a743464dff4d9c60fd4480d1c1e73dd819a9a67f1078815dce9d",
        ),
        ("macos", "x86_64") => (
            "darwin-x64",
            "abb6a7a895aaf2cc0df36fecc3bf0042479ffec51a643a056d54e9109f27db55",
            "macos-x64",
            "7922e0953f2110c05976e3bf58f14e643d90427575e766b7d433f5f80cbee7e1",
        ),
        ("linux", "x86_64") => (
            "linux-x64",
            "9573bb2233aab0f9ea7647d5cca9726113cc1768de61d66b17267f4db84488f6",
            "linux-x64",
            "dc61b3ac6b8c9ca874c0cc4c57b2409791a64c5540404ca5f5367360babc313a",
        ),
        ("linux", "aarch64") => (
            "linux-arm64",
            "a96dbfa41d3ef5dbd1ef22b1c10d5187be9267e86093a870f06402a7ec931596",
            "linux-arm64",
            "55fd0b241214eff3de1e8ee4f22796662f2d2e7a49bcfca7477cfd0bac398195",
        ),
        // No Windows pins yet; add them with the .exe member names.
        _ => {
            return Err(Error(format!(
                "no pinned esbuild and Tailwind for {os}/{arch}; use `--offline`"
            )));
        }
    };
    Ok([
        Pin {
            name: "esbuild",
            version: ESBUILD,
            url: format!("https://registry.npmjs.org/@esbuild/{package}/-/{package}-{ESBUILD}.tgz"),
            sha256: esbuild,
            member: Some("package/bin/esbuild"),
        },
        Pin {
            name: "tailwindcss",
            version: TAILWIND,
            url: format!(
                "https://github.com/tailwindlabs/tailwindcss/releases/download/v{TAILWIND}/tailwindcss-{asset}"
            ),
            sha256: tailwind,
            member: None,
        },
    ])
}

/// Lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The per-user cache directory.
pub fn cache_dir() -> Result<PathBuf, Error> {
    let var = |name| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if let Some(dir) = var("GRIFFIN_CACHE_DIR") {
        return Ok(dir);
    } else if let Some(dir) = var("XDG_CACHE_HOME") {
        dir
    } else if let Some(home) = var("HOME") {
        if cfg!(target_os = "macos") {
            home.join("Library/Caches")
        } else {
            home.join(".cache")
        }
    } else {
        return Err(Error(
            "cannot find a cache directory: set HOME or GRIFFIN_CACHE_DIR".into(),
        ));
    };
    Ok(base.join("griffin"))
}

/// The verified executable for `pin` in `cache`, downloading it first if it is not there.
///
/// The file is hashed before anything reads it as more than bytes. The cache entry is
/// named by the checksum and is only ever created, by one rename, after the check
/// passed, so a directory that exists holds a verified tool.
pub fn ensure(pin: &Pin, cache: &Path) -> Result<PathBuf, Error> {
    let entry = cache.join(format!(
        "{}-{}-{}",
        pin.name,
        pin.version,
        &pin.sha256[..12]
    ));
    let binary = entry.join(pin.name);
    if binary.is_file() {
        return Ok(binary);
    }
    let fail = |what: &str, error: &dyn std::fmt::Display| {
        Error(format!("{} {}: {what}: {error}", pin.name, pin.version))
    };
    fs::create_dir_all(cache).map_err(|e| fail("cannot create the cache", &e))?;
    let stage = cache.join(format!(".staging-{}-{}", pin.name, std::process::id()));
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage).map_err(|e| fail("cannot create the cache", &e))?;
    let result = download_into(pin, &stage).and_then(|()| match fs::rename(&stage, &entry) {
        Ok(()) => Ok(()),
        // Another process installed it first: its entry is verified too.
        Err(_) if binary.is_file() => Ok(()),
        Err(e) => Err(fail("cannot install", &e)),
    });
    let _ = fs::remove_dir_all(&stage);
    result.map(|()| binary)
}

fn download_into(pin: &Pin, stage: &Path) -> Result<(), Error> {
    let fail = |what: &str, error: &dyn std::fmt::Display| {
        Error(format!("{} {}: {what}: {error}", pin.name, pin.version))
    };
    let file = stage.join("download");
    let status = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--proto", "=https,file", "--proto-redir", "=https"])
        .arg("--output")
        .arg(&file)
        .arg(&pin.url)
        .status()
        .map_err(|e| fail("cannot run curl", &e))?;
    if !status.success() {
        return Err(fail("download failed", &pin.url));
    }
    let mut hasher = Sha256::new();
    let mut source = fs::File::open(&file).map_err(|e| fail("cannot read the download", &e))?;
    let mut chunk = vec![0u8; 1 << 16];
    loop {
        let read = io::Read::read(&mut source, &mut chunk)
            .map_err(|e| fail("cannot read the download", &e))?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
    }
    let found = hex(&hasher.finalize());
    if found != pin.sha256 {
        return Err(fail(
            "checksum mismatch, nothing was installed",
            &format!("expected {}, got {found}", pin.sha256),
        ));
    }
    let binary = stage.join(pin.name);
    match pin.member {
        None => fs::rename(&file, &binary).map_err(|e| fail("cannot install", &e))?,
        Some(member) => {
            let status = Command::new("tar")
                .arg("-xzf")
                .arg(&file)
                .arg("-C")
                .arg(stage)
                .arg(member)
                .status()
                .map_err(|e| fail("cannot run tar", &e))?;
            if !status.success() {
                return Err(fail("cannot unpack", &pin.url));
            }
            fs::rename(stage.join(member), &binary).map_err(|e| fail("cannot install", &e))?;
        }
    }
    // What is left in the entry is the executable alone.
    let _ = fs::remove_file(&file);
    let _ = fs::remove_dir_all(stage.join("package"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))
            .map_err(|e| fail("cannot make it executable", &e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("griffin-tools-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A local file standing in for a download, and a pin for it.
    fn fixture(dir: &Path, bytes: &[u8], sha256: Option<&'static str>) -> Pin {
        let source = dir.join("tool-source");
        fs::write(&source, bytes).unwrap();
        let real: &'static str = Box::leak(hex(&Sha256::digest(bytes)).into_boxed_str());
        Pin {
            name: "tool",
            version: "1.0.0",
            url: format!("file://{}", source.display()),
            sha256: sha256.unwrap_or(real),
            member: None,
        }
    }

    #[test]
    fn a_download_that_matches_its_checksum_is_cached_and_not_fetched_twice() {
        let dir = scratch("good");
        let cache = dir.join("cache");
        let pin = fixture(&dir, b"#!/bin/sh\necho hi\n", None);
        let binary = ensure(&pin, &cache).unwrap();
        assert_eq!(fs::read(&binary).unwrap(), b"#!/bin/sh\necho hi\n");
        // The source is gone, so a second call can only be served from the cache.
        fs::remove_file(dir.join("tool-source")).unwrap();
        assert_eq!(ensure(&pin, &cache).unwrap(), binary);
    }

    #[test]
    fn a_download_with_the_wrong_checksum_installs_nothing() {
        let dir = scratch("bad");
        let cache = dir.join("cache");
        let wrong = "0".repeat(64);
        let pin = fixture(&dir, b"tampered", Some(Box::leak(wrong.into_boxed_str())));
        let error = ensure(&pin, &cache).unwrap_err().to_string();
        assert!(error.contains("checksum mismatch"), "{error}");
        let left: Vec<_> = fs::read_dir(&cache).unwrap().collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn a_tarball_member_is_unpacked_only_after_the_check() {
        let dir = scratch("tgz");
        fs::create_dir_all(dir.join("src/package/bin")).unwrap();
        fs::write(dir.join("src/package/bin/tool"), "binary").unwrap();
        let tgz = dir.join("tool.tgz");
        let made = Command::new("tar")
            .arg("-czf")
            .arg(&tgz)
            .arg("-C")
            .arg(dir.join("src"))
            .arg("package")
            .status()
            .unwrap();
        assert!(made.success());
        let bytes = fs::read(&tgz).unwrap();
        let mut pin = fixture(&dir, &bytes, None);
        pin.member = Some("package/bin/tool");
        let binary = ensure(&pin, &dir.join("cache")).unwrap();
        assert_eq!(fs::read(binary).unwrap(), b"binary");
    }

    #[test]
    fn every_supported_platform_has_both_pins_with_https_urls() {
        for (os, arch) in [
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("linux", "x86_64"),
            ("linux", "aarch64"),
        ] {
            for pin in pins(os, arch).unwrap() {
                assert!(pin.url.starts_with("https://"), "{}", pin.url);
                assert_eq!(pin.sha256.len(), 64);
            }
        }
        assert!(pins("windows", "x86_64").is_err());
    }
}
